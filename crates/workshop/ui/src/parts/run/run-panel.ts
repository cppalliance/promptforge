// The Run window panel: one window per instance, in the main zone. The
// state machine is empty (no prompt chosen: the prompt field, Browse,
// and the drop target), loading (the tab title shimmers through
// run-tab.ts; the body stays blank), ready (the contract rows and the
// enabled Run button, whose click handler is deliberately empty in this
// slice), and error (the parser's or transport's message as one row,
// with Choose Prompt). A generation counter discards a superseded load,
// so a fast Choose-Prompt-then-drop sequence never renders stale rows.
//
// The panel never reads disk: prompt text arrives through the confined
// GET /workspace/file and is posted to POST /prompts/contract. Prompts
// arrive three ways: Browse (the native picker filtered to .md in the
// desktop app, a typed-path dialog in a plain browser, then the
// existing grant flow), a drag from the workspace tree (the
// application/x-workshop-path payload; the tree only hands out granted
// paths), and an OS drop (granted by workspace-drops before the
// workshop:file-drop dispatch reaches this panel; the first .md wins).

import { open } from "@tauri-apps/plugin-dialog";
import type { DockviewPanelApi, GroupPanelPartInitParameters } from "dockview";

import { toDisposable } from "../../base/lifecycle";
import { WorkshopPart } from "../../base/workshop-part";
import { errorText } from "../../services/error-catalog";
import { fetchPromptContract, type RunContract } from "../../services/run-api";
import { getServiceOrNull } from "../../services/service-registry";
import { fetchFile } from "../../services/workspace-api";
import { showPanelDialog } from "../shared/panel-dialog";
import { setRunTabLoading } from "../layout/run-tab";
import { STATUS_BAR } from "../../services/status-bar";
import { grantPath, WORKSPACE_FILE_DROP_EVENT } from "../workspace/workspace-drops";
import { renderContractRows } from "./run-rows";
import "./run-panel.css";

/** The dataTransfer type the workspace tree's drag-out sets. */
const WORKSHOP_PATH_MIME = "application/x-workshop-path";

/** The panel's states. */
type RunState = "empty" | "loading" | "ready" | "error";

/** The file's base name, for the contract request and the tab title. */
function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export class RunPanel extends WorkshopPart {
  private state: RunState = "empty";
  private generation = 0;
  private panelId: string | null = null;
  private panelApi: DockviewPanelApi | null = null;
  private contract: RunContract | null = null;
  private failure: string | null = null;
  private promptPath: string | null = null;
  // The open Choose Prompt / input Browse dialog, dismissed with the panel.
  private dialog: { dispose(): void } | null = null;

  private readonly toolbar = document.createElement("div");
  private readonly promptField = document.createElement("input");
  private readonly body = document.createElement("div");
  private readonly footer = document.createElement("div");
  private readonly runButton = document.createElement("button");

  constructor() {
    super();
    this.element.className = "ws-run-panel";
    // The whole panel is the OS-drop target; workspace-drops dispatches
    // workshop:file-drop here after granting the dropped paths.
    this.element.setAttribute("data-ws-file-drop", "");
  }

  override init(parameters: GroupPanelPartInitParameters): void {
    this.panelId = parameters.api.id;
    this.panelApi = parameters.api;
    super.init(parameters);
    const path = parameters.params?.path;
    if (typeof path === "string" && path.length > 0) {
      void this.load(path);
    }
  }

  protected create(parent: HTMLElement): void {
    this.toolbar.className = "ws-run-panel__toolbar";
    const promptLabel = document.createElement("label");
    promptLabel.className = "ws-run-panel__prompt-label";
    promptLabel.textContent = "Prompt";
    this.promptField.type = "text";
    this.promptField.className = "input ws-run-panel__prompt-path";
    this.promptField.placeholder = "No prompt chosen";
    this.promptField.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && this.promptField.value.trim().length > 0) {
        void this.grantAndLoad(this.promptField.value.trim());
      }
    });
    const browse = document.createElement("button");
    browse.type = "button";
    browse.className = "button button-outline button-sm ws-run-panel__browse";
    browse.textContent = "Browse...";
    browse.addEventListener("click", () => this.browsePrompt());
    this.toolbar.append(promptLabel, this.promptField, browse);

    this.body.className = "ws-run-panel__body";

    this.footer.className = "ws-run-panel__footer";
    this.runButton.type = "button";
    this.runButton.className = "button button-primary ws-run-panel__run";
    this.runButton.textContent = "Run";
    this.runButton.disabled = true;
    // Execution is out of scope for this slice: the button exists so the
    // window's shape is final, and does nothing when clicked.
    this.runButton.addEventListener("click", () => undefined);
    this.footer.appendChild(this.runButton);

    parent.append(this.toolbar, this.body, this.footer);

    // Tree drags: accept the workshop-path payload anywhere on the panel.
    const onDragOver = (event: DragEvent): void => {
      if (event.dataTransfer?.types.includes(WORKSHOP_PATH_MIME) === true) {
        event.preventDefault();
      }
    };
    this.element.addEventListener("dragover", onDragOver);
    const onDrop = (event: DragEvent): void => {
      if (event.dataTransfer?.types.includes(WORKSHOP_PATH_MIME) !== true) {
        return;
      }
      event.preventDefault();
      const path = event.dataTransfer.getData(WORKSHOP_PATH_MIME);
      if (path.length > 0) {
        void this.load(path);
      }
    };
    this.element.addEventListener("drop", onDrop);
    // OS drops arrive granted; the first .md path wins.
    const onFileDrop = (event: Event): void => {
      const detail: unknown = event instanceof CustomEvent ? event.detail : null;
      if (typeof detail !== "object" || detail === null || !("paths" in detail)) {
        return;
      }
      const { paths } = detail;
      if (!Array.isArray(paths)) {
        return;
      }
      const prompt = paths.find(
        (path): path is string => typeof path === "string" && path.toLowerCase().endsWith(".md"),
      );
      if (prompt !== undefined) {
        void this.load(prompt);
      }
    };
    this.element.addEventListener(WORKSPACE_FILE_DROP_EVENT, onFileDrop);
    this._register(
      toDisposable(() => {
        this.element.removeEventListener("dragover", onDragOver);
        this.element.removeEventListener("drop", onDrop);
        this.element.removeEventListener(WORKSPACE_FILE_DROP_EVENT, onFileDrop);
      }),
    );
    this.render();
  }

  override dispose(): void {
    this.dialog?.dispose();
    this.dialog = null;
    super.dispose();
  }

  /** Transitions the state machine, driving the tab's shimmer. */
  private setState(state: RunState): void {
    this.state = state;
    if (this.panelId !== null) {
      setRunTabLoading(this.panelId, state === "loading");
    }
    this.render();
  }

  /**
   * Loads one prompt file: reads its text through the confined file
   * route, posts the text to the parse route, and renders the contract.
   * A load that settles after a newer load started is discarded.
   */
  private async load(path: string): Promise<void> {
    const generation = ++this.generation;
    this.promptPath = path;
    this.setState("loading");
    const name = baseName(path);
    try {
      const file = await fetchFile(path);
      const contract = await fetchPromptContract(name, file.text);
      if (generation !== this.generation) {
        return;
      }
      this.contract = contract;
      this.failure = null;
      this.panelApi?.setTitle(`Run: ${name}`);
      this.setState("ready");
    } catch (error) {
      if (generation !== this.generation) {
        return;
      }
      this.contract = null;
      this.failure = errorText(error);
      this.setState("error");
    }
  }

  /**
   * The Browse flow: the native picker filtered to .md in the desktop
   * app, a typed-path dialog in a plain browser. The chosen path is
   * granted through the existing grant flow before it is read.
   */
  private browsePrompt(): void {
    if (window.__TAURI_INTERNALS__ !== undefined) {
      void open({
        directory: false,
        title: "Choose Prompt",
        filters: [{ name: "Prompt", extensions: ["md"] }],
      }).then((picked) => {
        if (typeof picked === "string") {
          void this.grantAndLoad(picked);
        }
      });
      return;
    }
    this.dialog?.dispose();
    this.dialog = showPanelDialog({
      host: this.element,
      classPrefix: "ws-run-choose",
      titleId: "run-choose-title",
      title: "Choose Prompt",
      message: "Enter the full path of a prompt file.",
      field: { id: "run-choose-path", label: "Prompt path" },
      buttons: [
        {
          label: "Choose",
          requiresValue: true,
          run: (value) => {
            void this.grantAndLoad(value);
          },
        },
        { label: "Cancel", run: () => undefined },
      ],
    });
  }

  /** Grants a chosen or typed path, then loads it. */
  private async grantAndLoad(path: string): Promise<void> {
    const result = await grantPath(path);
    if (!result.ok) {
      getServiceOrNull(STATUS_BAR)?.showLocal(
        `Could not open ${path}: ${result.error.message}`,
        "error",
      );
      return;
    }
    await this.load(path);
  }

  /** The input row's Browse: fills the field, reading nothing. */
  private browseInput(field: HTMLInputElement): void {
    if (window.__TAURI_INTERNALS__ !== undefined) {
      void open({ directory: false, title: "Choose Input" }).then((picked) => {
        if (typeof picked === "string") {
          field.value = picked;
        }
      });
      return;
    }
    this.dialog?.dispose();
    this.dialog = showPanelDialog({
      host: this.element,
      classPrefix: "ws-run-input",
      titleId: "run-input-title",
      title: "Choose Input",
      message: "Enter the full path of the input file.",
      field: { id: "run-input-path", label: "Input path" },
      buttons: [
        {
          label: "Choose",
          requiresValue: true,
          run: (value) => {
            field.value = value;
          },
        },
        { label: "Cancel", run: () => undefined },
      ],
    });
  }

  /** Paints the body's state-dependent content and the Run button. */
  private render(): void {
    this.promptField.value = this.promptPath ?? "";
    this.runButton.disabled = this.state !== "ready";
    this.body.textContent = "";
    if (this.state === "empty") {
      const hint = document.createElement("p");
      hint.className = "ws-run-panel__hint";
      hint.textContent = "Drop a prompt file here, or Browse to choose one.";
      this.body.appendChild(hint);
      return;
    }
    if (this.state === "loading") {
      // The loading signal is the tab title's shimmer; the body stays blank.
      return;
    }
    if (this.state === "error") {
      const error = document.createElement("div");
      error.className = "ws-run-panel__error";
      error.setAttribute("role", "alert");
      error.textContent = this.failure ?? "The prompt could not be parsed.";
      const choose = document.createElement("button");
      choose.type = "button";
      choose.className = "button button-outline button-sm ws-run-panel__choose";
      choose.textContent = "Choose Prompt";
      choose.addEventListener("click", () => this.browsePrompt());
      this.body.append(error, choose);
      return;
    }
    if (this.contract !== null) {
      renderContractRows(this.body, this.contract, {
        browseInput: (field) => this.browseInput(field),
      });
    }
  }
}
