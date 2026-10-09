// The Run window panel: one window per instance, in the main zone. The
// state machine is empty (no prompt chosen: the prompt field, Browse,
// and the drop target), loading (the tab title shimmers through
// setTabLoading; the body stays blank), ready (the contract rows and the
// enabled Run button, whose click handler is deliberately empty in this
// slice), and error (the parser's or transport's message as one row,
// with Choose Prompt). A generation counter discards a superseded load,
// so a fast Choose-Prompt-then-drop sequence never renders stale rows.
//
// The layout copies Cursor Settings: the toolbar is a 35px strip whose
// "Prompt" label is for screen readers only, the empty and error states
// are text lines with Browse... and Choose Prompt as inline links, a
// droppable drag marks the panel with a drag-over overlay class, and the
// typed-path dialogs (Choose Prompt, Choose Input) are Cursor's form
// modal, Cancel first.
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
import type { GroupPanelPartInitParameters } from "dockview";

import { toDisposable } from "@workshop/platform/lifecycle";
import { WorkshopPart } from "@workshop/platform/workshop-part";
import { errorText } from "../../services/error-catalog";
import { fetchPromptContract, type RunContract } from "../../services/run-api";
import { getServiceOrNull } from "@workshop/platform/service-registry";
import { fetchFile } from "../../services/workspace-api";
import { showPanelDialog } from "../shared/panel-dialog";
import { setTabLoading } from "../layout/panel-tab";
import { STATUS_BAR } from "@workshop/platform/status-bar";
import { grantPath, WORKSPACE_FILE_DROP_EVENT } from "../workspace/workspace-drops";
import { renderContractRows } from "./run-rows";
import "./run-panel.css";

/** The dataTransfer type the workspace tree's drag-out sets. */
const WORKSHOP_PATH_MIME = "application/x-workshop-path";

/** The class a droppable drag puts on the panel while it hovers; the overlay is its ::after. */
const DRAG_OVER_CLASS = "ws-run-panel--drag-over";

/** The path field's id counter: each window's label needs an id of its own. */
let promptFieldCount = 0;

/** True when the drag holds a workspace-tree path or OS files, the two things the panel accepts. */
function isDroppableDrag(event: DragEvent): boolean {
  const types = event.dataTransfer?.types;
  if (types === undefined) {
    return false;
  }
  const held = Array.from(types);
  return held.includes(WORKSHOP_PATH_MIME) || held.includes("Files");
}

/** An inline text-link button: the Cursor-style link that sits in a line of text. */
function linkButton(text: string, extraClass: string, onClick: () => void): HTMLButtonElement {
  const link = document.createElement("button");
  link.type = "button";
  link.className = `ws-run-panel__link ${extraClass}`;
  link.textContent = text;
  link.addEventListener("click", onClick);
  return link;
}

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
  private contract: RunContract | null = null;
  private failure: string | null = null;
  private promptPath: string | null = null;
  // The open Choose Prompt / input Browse dialog, dismissed with the panel.
  private dialog: { dispose(): void } | null = null;

  // Holds the toolbar, body, and footer. It is the width container the rows
  // stack against, so its width is the panel's. The panel's dialogs mount in
  // the panel element beside it, outside the container, which would
  // otherwise be the containing block for their fixed overlay.
  private readonly content = document.createElement("div");
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
    super.init(parameters);
    const path = parameters.params?.path;
    if (typeof path === "string" && path.length > 0) {
      void this.load(path);
    }
  }

  protected create(parent: HTMLElement): void {
    this.toolbar.className = "ws-run-panel__toolbar";
    // The label is read by screen readers only; the toolbar shows the field.
    const promptLabel = document.createElement("label");
    promptLabel.className = "ws-run-panel__prompt-label";
    promptLabel.textContent = "Prompt";
    promptFieldCount += 1;
    this.promptField.id = `ws-run-prompt-path-${promptFieldCount}`;
    promptLabel.htmlFor = this.promptField.id;
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

    this.content.className = "ws-run-panel__content";
    this.content.append(this.toolbar, this.body, this.footer);
    parent.appendChild(this.content);

    // Tree drags: accept the workshop-path payload anywhere on the panel.
    const onDragOver = (event: DragEvent): void => {
      if (event.dataTransfer?.types.includes(WORKSHOP_PATH_MIME) === true) {
        event.preventDefault();
      }
    };
    this.element.addEventListener("dragover", onDragOver);
    // The drag-over mark: dragenter and dragleave fire for every child the
    // pointer crosses, so a depth count keeps the class up until the drag
    // has left the panel. A drop, or a drag that ends anywhere, resets it.
    let dragDepth = 0;
    const clearDragOver = (): void => {
      dragDepth = 0;
      this.element.classList.remove(DRAG_OVER_CLASS);
    };
    const onDragEnter = (event: DragEvent): void => {
      if (isDroppableDrag(event)) {
        dragDepth += 1;
        this.element.classList.add(DRAG_OVER_CLASS);
      }
    };
    const onDragLeave = (event: DragEvent): void => {
      if (isDroppableDrag(event)) {
        dragDepth = Math.max(0, dragDepth - 1);
        if (dragDepth === 0) {
          this.element.classList.remove(DRAG_OVER_CLASS);
        }
      }
    };
    this.element.addEventListener("dragenter", onDragEnter);
    this.element.addEventListener("dragleave", onDragLeave);
    window.addEventListener("dragend", clearDragOver);
    const onDrop = (event: DragEvent): void => {
      clearDragOver();
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
        this.element.removeEventListener("dragenter", onDragEnter);
        this.element.removeEventListener("dragleave", onDragLeave);
        window.removeEventListener("dragend", clearDragOver);
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
      setTabLoading(this.panelId, state === "loading");
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
      container: this.element,
      classPrefix: "ws-run-choose",
      titleId: "run-choose-title",
      title: "Choose Prompt",
      message: "Enter the full path of a prompt file.",
      skin: "form",
      field: { id: "run-choose-path", label: "Prompt path" },
      buttons: [
        { label: "Cancel", run: () => undefined },
        {
          label: "Choose",
          primary: true,
          requiresValue: true,
          run: (value) => {
            void this.grantAndLoad(value);
          },
        },
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
      container: this.element,
      classPrefix: "ws-run-input",
      titleId: "run-input-title",
      title: "Choose Input",
      message: "Enter the full path of the input file.",
      skin: "form",
      field: { id: "run-input-path", label: "Input path" },
      buttons: [
        { label: "Cancel", run: () => undefined },
        {
          label: "Choose",
          primary: true,
          requiresValue: true,
          run: (value) => {
            field.value = value;
          },
        },
      ],
    });
  }

  /** Paints the body's state-dependent content and the Run button. */
  private render(): void {
    this.promptField.value = this.promptPath ?? "";
    this.runButton.disabled = this.state !== "ready";
    this.body.textContent = "";
    if (this.state === "empty") {
      // One line of text that ends in the Browse... link.
      const hint = document.createElement("p");
      hint.className = "ws-run-panel__message ws-run-panel__hint";
      hint.append(
        "Drop a prompt file here, or ",
        linkButton("Browse...", "ws-run-panel__browse-link", () => this.browsePrompt()),
      );
      this.body.appendChild(hint);
      return;
    }
    if (this.state === "loading") {
      // The loading signal is the tab title's shimmer; the body stays blank.
      return;
    }
    if (this.state === "error") {
      // The alert holds only the message; Choose Prompt follows it inline.
      const message = document.createElement("div");
      message.className = "ws-run-panel__message";
      const error = document.createElement("span");
      error.className = "ws-run-panel__error";
      error.setAttribute("role", "alert");
      error.textContent = this.failure ?? "The prompt could not be parsed.";
      message.append(
        error,
        " ",
        linkButton("Choose Prompt", "ws-run-panel__choose", () => this.browsePrompt()),
      );
      this.body.appendChild(message);
      return;
    }
    if (this.contract !== null) {
      renderContractRows(this.body, this.contract, {
        browseInput: (field) => this.browseInput(field),
      });
    }
  }
}
