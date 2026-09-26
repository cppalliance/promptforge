// The editor panel: one open document per Dockview panel, written against
// the EditorSurface contract. The panel owns the document lifecycle -
// loading through the workspace API, dirty state in the tab title, and
// saving with the server's opaque conflict token - and never
// touches the concrete editor. A save that loses the token race opens a
// themed conflict dialog (reload the on-disk text, or overwrite it)
// instead of silently clobbering the file.

import "./editor-panel.css";

import type { DockviewPanelApi, GroupPanelPartInitParameters } from "dockview";
import type { EditorView } from "@codemirror/view";

import { Emitter } from "../../base/event";
import { baseName } from "../../base/paths";
import { toDisposable } from "../../base/lifecycle";
import { WorkshopPart } from "../../base/workshop-part";
import { Commands } from "../../services/command-registry";
import { errorText } from "../../services/error-catalog";
import { RECENT_FILES_STORE } from "../../services/recent-files-store";
import { getServiceOrNull } from "../../services/service-registry";
import { showPanelDialog } from "./editor-dialog";
import { CodeMirrorSurface, languageIdForPath, type EditorSurface } from "./editor-surface";
import {
  fetchFile,
  isDeadlineElapsed,
  isModifiedConflict,
  writeFile,
  type WorkspaceFile,
} from "../../services/workspace-api";

/** Injectable seams for tests; production uses the real surface and API. */
export interface EditorPanelDeps {
  readonly createSurface?: () => EditorSurface;
  readonly readFile?: (path: string) => Promise<WorkspaceFile>;
  readonly writeFile?: (
    path: string,
    text: string,
    expectedToken: string | null,
  ) => Promise<WorkspaceFile>;
}

/** Reads the file path out of panel params, which arrive as unknown fields. */
function filePathParam(params: Record<string, unknown>): string | null {
  const path = params.path;
  return typeof path === "string" && path.length > 0 ? path : null;
}

/**
 * Reads the untitled serial out of panel params: a positive integer
 * marks the panel as an untitled buffer and numbers its Untitled-N
 * title and its panel id.
 */
function untitledSerialParam(params: Record<string, unknown>): number | null {
  const untitled = params.untitled;
  return typeof untitled === "number" && Number.isInteger(untitled) && untitled > 0 ? untitled : null;
}

const SAVE_TIMEOUT_MESSAGE = "The save timed out; the file may or may not have been written.";

const didInitEmitter = new Emitter<EditorPanel>();

/**
 * Fires as each EditorPanel finishes init. The lifecycle module's
 * context-key binder hooks it: the lazy chunk swap that mounts the real
 * panel fires no dock event, so dock subscriptions alone would miss it.
 */
export const onDidInitEditorPanel = didInitEmitter.event;

export class EditorPanel extends WorkshopPart {
  private readonly surface: EditorSurface;
  private panelApi: DockviewPanelApi | null = null;
  private path: string | null = null;
  private untitled = false;
  private title = "Editor";
  private token: string | null = null;
  /** True while a timed-out save leaves the token unknown. */
  private tokenUnknown = false;
  /** The text of the last write attempt, for reconciling an unknown token. */
  private lastSentText: string | null = null;
  private saving = false;

  constructor(private readonly deps: EditorPanelDeps = {}) {
    super();
    this.element.className = "ws-editor-panel";
    // The surface is the panel's child: dockview disposes the panel when
    // its tab closes, and the inherited dispose() releases the surface and
    // the dirty subscription with it.
    this.surface = this._register(deps.createSurface?.() ?? new CodeMirrorSurface());
    this._register(
      toDisposable(
        this.surface.onDirtyChange(() => {
          this.updateTitle();
        }),
      ),
    );
  }

  protected create(parent: HTMLElement): void {
    parent.appendChild(this.surface.element);
  }

  override init(parameters: GroupPanelPartInitParameters): void {
    super.init(parameters);
    this.panelApi = parameters.api;
    const path = filePathParam(parameters.params);
    if (path !== null) {
      this.path = path;
      this.title = baseName(path);
      this.updateTitle();
      void this.load(path)
        .then(() => {
          // Every opened path feeds File > Open Recent and quick open.
          getServiceOrNull(RECENT_FILES_STORE)?.add(path);
        })
        .catch((error: unknown) => {
          this.showError(error);
        });
    } else {
      const serial = untitledSerialParam(parameters.params);
      if (serial === null) {
        this.showError("No file path was provided for this editor.");
        return;
      }
      // An untitled buffer: no read, no write target - save runs Save As.
      this.untitled = true;
      this.title = `Untitled-${serial}`;
      this.updateTitle();
      const text = typeof parameters.params.text === "string" ? parameters.params.text : "";
      this.surface.open({ path: "", text });
      if (text !== "") {
        // A restored untitled buffer (Reopen Closed Editor) is dirty
        // against the empty baseline: its content was never persisted.
        this.surface.markSaved("");
      }
    }
    didInitEmitter.fire(this);
  }

  /** The panel's file path, or null for an untitled buffer. */
  filePath(): string | null {
    return this.path;
  }

  /** Whether the panel is an untitled buffer (no path; save runs Save As). */
  isUntitled(): boolean {
    return this.untitled;
  }

  /** The live editor text - what a save or an untitled reopen writes. */
  currentText(): string {
    return this.surface.text();
  }

  /** The document's language id, for the editorLangId context key. */
  languageId(): string {
    return this.path === null ? "plaintext" : languageIdForPath(this.path);
  }

  /** The panel's dirty state, for close prompts and save shortcuts. */
  isDirty(): boolean {
    return this.surface.isDirty();
  }

  /** The surface's live EditorView, for the editor command layer. */
  editorView(): EditorView | null {
    return this.surface.editorView();
  }

  focus(): void {
    this.surface.focus();
  }

  /**
   * Saves through the workspace API with the token from the last read.
   * A stale token means the file changed on disk: rather than overwriting
   * silently, the conflict dialog offers reload or overwrite. A timed-out
   * save (a 408) leaves the token unknown - the write may or may not have
   * landed - so the next save re-reads the file before sending any token,
   * adopting the fresh token when the disk still holds what was last sent,
   * and falling back to the conflict dialog otherwise. The write itself
   * routes through writeCurrent, the same path overwrite() uses, so both
   * record the 408 and 409 outcomes identically. An untitled buffer has no
   * write target, so its save runs Save As, which resolves this panel
   * through the dock's active panel.
   */
  async save(): Promise<void> {
    if (this.path === null) {
      if (this.untitled) {
        await Commands.execute("workbench.action.files.saveAs");
      }
      return;
    }
    if (this.saving) {
      return;
    }
    this.saving = true;
    // The text is captured once: the write and the saved baseline must
    // agree, or keystrokes typed while the PUT is in flight would be
    // baselined as saved and silently lost.
    const text = this.surface.text();
    try {
      // A timed-out save left the token unknown: reconcile with the file
      // on disk before sending any token, so a stale token never reaches
      // the write boundary.
      let expectedToken = this.token;
      if (this.tokenUnknown) {
        const onDisk = await this.reader()(this.path);
        if (onDisk.text !== this.lastSentText) {
          // The write may not have landed, or the file changed again:
          // resolve through the conflict dialog instead of overwriting.
          this.showConflictDialog(
            `${this.title} may hold an earlier timed-out save or an outside edit. Reload the on-disk text, or overwrite the file with your changes.`,
          );
          return;
        }
        this.token = onDisk.token;
        expectedToken = onDisk.token;
        this.tokenUnknown = false;
      }
      await this.writeCurrent(this.path, text, expectedToken);
    } catch (error: unknown) {
      if (isDeadlineElapsed(error)) {
        this.tokenUnknown = true;
        this.showError(SAVE_TIMEOUT_MESSAGE);
      } else if (isModifiedConflict(error)) {
        this.showConflictDialog();
      } else {
        this.showError(error);
      }
    } finally {
      this.saving = false;
    }
  }

  /**
   * Save As: writes the live text to a new path and retargets the panel
   * onto it - the tab title, the conflict token, and the saved baseline
   * all move to the new file, and an untitled buffer becomes a file
   * editor. The picker and the parent-directory grant are the caller's
   * job (the workspace contribution). Every failure, a 408 included,
   * paints the error bar and leaves the panel on its old path; the
   * conflict dialog never opens, because it acts on that old path.
   */
  async saveAs(path: string): Promise<void> {
    if (this.saving) {
      return;
    }
    this.saving = true;
    try {
      // The text is captured once, as in save(): the write and the saved
      // baseline must agree.
      const text = this.surface.text();
      const written = await this.writeTarget(path, text);
      if (written === null) {
        return;
      }
      this.path = path;
      this.untitled = false;
      this.title = baseName(path);
      this.token = written.token;
      this.tokenUnknown = false;
      this.surface.markSaved(text);
      this.updateTitle();
      getServiceOrNull(RECENT_FILES_STORE)?.add(path);
    } catch (error: unknown) {
      this.showError(isDeadlineElapsed(error) ? SAVE_TIMEOUT_MESSAGE : error);
    } finally {
      this.saving = false;
    }
  }

  /**
   * Revert File: reloads the on-disk text. A dirty panel prompts first -
   * reverting discards its unsaved changes; a clean panel reloads
   * immediately; an untitled buffer has no on-disk text and ignores the
   * command.
   */
  requestRevert(): void {
    const path = this.path;
    if (path === null) {
      return;
    }
    if (!this.surface.isDirty()) {
      void this.load(path).catch((error: unknown) => {
        this.showError(error);
      });
      return;
    }
    this._register(
      showPanelDialog({
        host: this.element,
        classPrefix: "ws-editor-revert",
        titleId: "editor-revert-title",
        title: "Revert file",
        message: `${this.title} has unsaved changes. Reverting to the on-disk text discards them.`,
        buttons: [
          {
            label: "Revert",
            danger: true,
            run: () => {
              void this.load(path).catch((error: unknown) => {
                this.showError(error);
              });
            },
          },
          { label: "Cancel", run: () => undefined },
        ],
      }),
    );
  }

  private reader(): (path: string) => Promise<WorkspaceFile> {
    return this.deps.readFile ?? fetchFile;
  }
  private writer(): (
    path: string,
    text: string,
    expectedToken: string | null,
  ) => Promise<WorkspaceFile> {
    return this.deps.writeFile ?? writeFile;
  }

  /**
   * Writes a Save As target. The picker's replace confirmation is the
   * user's consent, so a 409 on an existing target re-reads its token and
   * retries once. An unreadable target or a second conflict paints an
   * error naming the target and returns null; a 408 or any other failure
   * is rethrown. The writes bypass writeCurrent: a 408 here leaves the
   * open file's token untouched.
   */
  private async writeTarget(path: string, text: string): Promise<WorkspaceFile | null> {
    try {
      return await this.writer()(path, text, null);
    } catch (error: unknown) {
      if (!isModifiedConflict(error)) {
        throw error;
      }
    }
    let target: WorkspaceFile;
    try {
      target = await this.reader()(path);
    } catch (error: unknown) {
      this.showError(`Save As could not replace ${path}: ${errorText(error)}`);
      return null;
    }
    try {
      return await this.writer()(path, text, target.token);
    } catch (error: unknown) {
      if (isModifiedConflict(error)) {
        this.showError(`Save As could not replace ${path}: it changed on disk again before the write.`);
        return null;
      }
      throw error;
    }
  }

  /**
   * Writes this panel's own file and records the outcome: the token, the
   * saved baseline, the text a timed-out write may have landed, the 408
   * message, and the 409 conflict dialog. Any other error is rethrown. A
   * 408 marks `this.path`'s token unknown, so a write to any other path
   * (Save As) must not route through here.
   */
  private async writeCurrent(path: string, text: string, expectedToken: string | null): Promise<void> {
    this.lastSentText = text;
    try {
      const written = await this.writer()(path, text, expectedToken);
      this.token = written.token;
      this.tokenUnknown = false;
      this.surface.markSaved(text);
    } catch (error: unknown) {
      if (isDeadlineElapsed(error)) {
        this.tokenUnknown = true;
        this.showError(SAVE_TIMEOUT_MESSAGE);
      } else if (isModifiedConflict(error)) {
        this.showConflictDialog();
      } else {
        throw error;
      }
    }
  }

  /** Loads the document into the surface and records its conflict token. */
  private async load(path: string): Promise<void> {
    const file = await this.reader()(path);
    this.token = file.token;
    this.tokenUnknown = false;
    this.surface.open({ path, text: file.text });
  }

  private updateTitle(): void {
    const dirty = this.surface.isDirty();
    this.panelApi?.setTitle(dirty ? `● ${this.title}` : this.title);
  }

  /** Paints a failure as a bar above the editor; the next error replaces it. */
  private showError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    this.element.querySelector(".ws-editor-panel__error")?.remove();
    const bar = document.createElement("p");
    bar.className = "ws-editor-panel__error";
    bar.setAttribute("role", "alert");
    bar.textContent = message;
    this.element.prepend(bar);
  }

  /**
   * The write-conflict modal. Reload discards the editor's text
   * for the on-disk text; Overwrite re-reads the fresh token and writes
   * the editor's text over the file.
   */
  private showConflictDialog(
    message = `${this.title} was modified outside the editor. Reload the on-disk text, or overwrite the file with your changes.`,
  ): void {
    this._register(showPanelDialog({
      host: this.element,
      classPrefix: "ws-editor-conflict",
      titleId: "editor-conflict-title",
      title: "File changed on disk",
      message,
      buttons: [
        {
          label: "Reload",
          run: () => {
            if (this.path !== null) {
              void this.load(this.path).catch((error: unknown) => {
                this.showError(error);
              });
            }
          },
        },
        {
          label: "Overwrite",
          danger: true,
          run: () => {
            void this.overwrite().catch((error: unknown) => {
              this.showError(error);
            });
          },
        },
      ],
    }));
  }

  /**
   * Close entry point for the Ctrl+W shortcut: a clean panel closes
   * immediately; a dirty panel opens the unsaved-changes dialog instead
   * of silently losing edits.
   */
  requestClose(): void {
    if (!this.surface.isDirty()) {
      this.panelApi?.close();
      return;
    }
    this.showCloseDialog();
  }

  /**
   * The unsaved-changes modal. Save writes and closes only when the write
   * succeeds; Discard closes without writing; Cancel keeps the panel.
   */
  private showCloseDialog(): void {
    this._register(showPanelDialog({
      host: this.element,
      classPrefix: "ws-editor-close",
      titleId: "editor-close-title",
      title: "Unsaved changes",
      message: `${this.title} has unsaved changes. Save before closing, or discard them.`,
      buttons: [
        {
          label: "Save",
          run: () => {
            void this.save()
              .then(() => {
                // A failed or conflicted save leaves the panel open.
                if (!this.surface.isDirty()) {
                  this.panelApi?.close();
                }
              })
              .catch((error: unknown) => {
                this.showError(error);
              });
          },
        },
        {
          label: "Discard",
          danger: true,
          run: () => {
            this.panelApi?.close();
          },
        },
        { label: "Cancel", run: () => undefined },
      ],
    }));
  }

  /**
   * Overwrite path of the conflict dialog: re-read the file for its fresh
   * token, then write the editor's text against it through writeCurrent,
   * the same path save() uses. A second conflict (the file changed again
   * in between) reopens the dialog, and a 408 leaves the token unknown
   * for the next save to reconcile.
   */
  private async overwrite(): Promise<void> {
    // The saving guard cannot wedge the conflict flow: the dialog's
    // Overwrite button runs on a later click, after save()'s finally
    // block has already cleared the flag.
    if (this.path === null || this.saving) {
      return;
    }
    this.saving = true;
    try {
      const fresh = await this.reader()(this.path);
      const text = this.surface.text();
      await this.writeCurrent(this.path, text, fresh.token);
    } catch (error: unknown) {
      if (isModifiedConflict(error)) {
        this.showConflictDialog();
      } else {
        this.showError(error);
      }
    } finally {
      this.saving = false;
    }
  }
}
