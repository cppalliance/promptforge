// The editor lifecycle: untitled-buffer allocation, the closed-editor
// tracking behind Reopen Closed Editor, and the editor-sourced context
// key (editorLangId) that menus and keybindings evaluate. activeEditor
// is the layout's: zones.ts binds it to the active panel's type id.
//
// The tracking and the key follow the dock; the directory's register()
// installs both when the editor chunk loads. The tracking records at
// onDidRemovePanel, where the panel's content is still resolved
// (dockview fires the event before disposing the renderer), onto the
// ClosedEditors service (closed-editors.ts, workspace-persisted and
// resolved through its token so the composition root can bind it to the
// live adapter). A reopened untitled buffer allocates a fresh serial so
// it never collides with a live one.

import type { DockviewApi, IDockviewPanel } from "dockview";

import { DisposableStore, type IDisposable } from "@workshop/platform/lifecycle";
import { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
import { getService } from "@workshop/platform/service-registry";
import { openInZone } from "../layout/zones";
import { CLOSED_EDITORS } from "../../services/closed-editors";
import { asEditor } from "./editor-commands";
import { onDidInitEditorPanel } from "./editor-panel";

let nextUntitledSerial = 1;

/** Allocates the serial behind an Untitled-N title and its panel id. */
export function allocateUntitledSerial(): number {
  const serial = nextUntitledSerial;
  nextUntitledSerial += 1;
  return serial;
}

/** Ctrl+N: opens a fresh untitled editor buffer. */
export function newUntitledFile(): void {
  openInZone("editor", { untitled: allocateUntitledSerial() });
}

/**
 * Records every closing editor on the closed-editor stack. Untitled
 * buffers keep their text for the session: reopening one restores the
 * content, still unsaved, though only file paths persist to the
 * workspace. The stack resolves per event, so a rebinding of the token
 * (a workspace switch) takes effect without reinstalling.
 */
export function installClosedEditorTracking(dock: DockviewApi): IDisposable {
  return dock.onDidRemovePanel((panel: IDockviewPanel) => {
    const editor = asEditor(panel);
    if (editor === null) {
      return;
    }
    const path = editor.filePath();
    getService(CLOSED_EDITORS).push(
      path !== null ? { kind: "file", path } : { kind: "untitled", text: editor.currentText() },
    );
  });
}

/** Ctrl+Shift+T: reopens the most recently closed editor; a no-op when none. */
export function reopenClosedEditor(): void {
  const closed = getService(CLOSED_EDITORS).pop();
  if (closed === undefined) {
    return;
  }
  if (closed.kind === "file") {
    openInZone("editor", { path: closed.path });
  } else {
    openInZone("editor", { untitled: allocateUntitledSerial(), text: closed.text });
  }
}

/**
 * Binds the editor-sourced context key to the dock: editorLangId is the
 * active editor's language, unset while no editor is active. Dock events
 * cover activation changes; the panel-init hook covers the lazy chunk
 * swap, which fires no dock event.
 */
export function bindEditorContextKeys(dock: DockviewApi): IDisposable {
  const context = getService(CONTEXT_KEY_SERVICE);
  const editorLangId = context.createKey<string | undefined>("editorLangId", undefined);
  const update = (): void => {
    const editor = asEditor(dock.activePanel);
    editorLangId.set(editor === null ? undefined : editor.languageId());
  };
  const store = new DisposableStore();
  store.add(dock.onDidActivePanelChange(update));
  store.add(dock.onDidAddPanel(update));
  store.add(onDidInitEditorPanel(update));
  update();
  return store;
}
