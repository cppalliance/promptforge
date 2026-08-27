// The editor panel: one open document per Dockview panel, written against
// the EditorSurface contract. The panel owns the document lifecycle -
// loading through the workspace API, dirty state in the tab title, and
// saving with the server's modified-time conflict token - and never
// touches the concrete editor. A save that loses the token race opens a
// themed conflict dialog (reload the on-disk text, or overwrite it)
// instead of silently clobbering the file.

import type { DockviewPanelApi, GroupPanelPartInitParameters, IContentRenderer } from "dockview";

import { showPanelDialog } from "./editor-dialog";
import { CodeMirrorSurface, type EditorSurface } from "./editor-surface";
import {
  fetchFile,
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
    expectedModifiedMs: number | null,
  ) => Promise<WorkspaceFile>;
}

/** Reads the file path out of panel params, which arrive as unknown fields. */
function filePathParam(params: Record<string, unknown>): string | null {
  const path = params.path;
  return typeof path === "string" && path.length > 0 ? path : null;
}

/** The file's base name, for the tab title. */
function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export class EditorPanel implements IContentRenderer {
  readonly element = document.createElement("div");
  private readonly surface: EditorSurface;
  private panelApi: DockviewPanelApi | null = null;
  private path: string | null = null;
  private title = "Editor";
  private modifiedMs: number | null = null;
  private saving = false;

  constructor(private readonly deps: EditorPanelDeps = {}) {
    this.element.className = "editor-panel";
    this.surface = deps.createSurface?.() ?? new CodeMirrorSurface();
    this.surface.onDirtyChange(() => {
      this.updateTitle();
    });
  }

  init(parameters: GroupPanelPartInitParameters): void {
    this.panelApi = parameters.api;
    this.element.appendChild(this.surface.element);
    const path = filePathParam(parameters.params);
    if (path === null) {
      this.showError("No file path was provided for this editor.");
      return;
    }
    this.path = path;
    this.title = baseName(path);
    this.updateTitle();
    void this.load(path).catch((error: unknown) => {
      this.showError(error);
    });
  }

  /** The panel's dirty state, for close prompts and save shortcuts. */
  isDirty(): boolean {
    return this.surface.isDirty();
  }

  focus(): void {
    this.surface.focus();
  }

  dispose(): void {
    this.surface.dispose();
  }

  /**
   * Saves through the workspace API with the token from the last read.
   * A stale token means the file changed on disk: rather than overwriting
   * silently, the conflict dialog offers reload or overwrite.
   */
  async save(): Promise<void> {
    if (this.path === null || this.saving) {
      return;
    }
    this.saving = true;
    try {
      const written = await this.writer()(this.path, this.surface.text(), this.modifiedMs);
      this.modifiedMs = written.modifiedMs;
      this.surface.markSaved();
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

  private reader(): (path: string) => Promise<WorkspaceFile> {
    return this.deps.readFile ?? fetchFile;
  }

  private writer(): (
    path: string,
    text: string,
    expectedModifiedMs: number | null,
  ) => Promise<WorkspaceFile> {
    return this.deps.writeFile ?? writeFile;
  }

  /** Loads the document into the surface and records its conflict token. */
  private async load(path: string): Promise<void> {
    const file = await this.reader()(path);
    this.modifiedMs = file.modifiedMs;
    this.surface.open({ path, text: file.text });
  }

  private updateTitle(): void {
    const dirty = this.surface.isDirty();
    this.panelApi?.setTitle(dirty ? `● ${this.title}` : this.title);
  }

  /** Paints a failure as a bar above the editor; the next error replaces it. */
  private showError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    this.element.querySelector(".editor-panel__error")?.remove();
    const bar = document.createElement("p");
    bar.className = "editor-panel__error";
    bar.setAttribute("role", "alert");
    bar.textContent = message;
    this.element.prepend(bar);
  }

  /**
   * The modified-time conflict modal. Reload discards the editor's text
   * for the on-disk text; Overwrite re-reads the fresh token and writes
   * the editor's text over the file.
   */
  private showConflictDialog(): void {
    showPanelDialog({
      host: this.element,
      classPrefix: "editor-conflict",
      titleId: "editor-conflict-title",
      title: "File changed on disk",
      message: `${this.title} was modified outside the editor. Reload the on-disk text, or overwrite the file with your changes.`,
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
    });
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
    showPanelDialog({
      host: this.element,
      classPrefix: "editor-close",
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
    });
  }

  /**
   * Overwrite path of the conflict dialog: re-read the file for its fresh
   * token, then write the editor's text against it. A second conflict
   * (the file changed again in between) reopens the dialog.
   */
  private async overwrite(): Promise<void> {
    if (this.path === null) {
      return;
    }
    const fresh = await this.reader()(this.path);
    try {
      const written = await this.writer()(this.path, this.surface.text(), fresh.modifiedMs);
      this.modifiedMs = written.modifiedMs;
      this.surface.markSaved();
    } catch (error: unknown) {
      if (isModifiedConflict(error)) {
        this.showConflictDialog();
      } else {
        this.showError(error);
      }
    }
  }
}
