// The editor feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { DOCK, registerPanelFactory } from "../../services/panel-registry";
import { getService } from "../../services/service-registry";
import { registerCommand } from "../menu/command-registry";
import { closeActiveEditor, cycleEditor, saveActiveEditor } from "./editor-commands";
import { bindEditorContextKeys, installClosedEditorTracking } from "./editor-lifecycle";
import { EditorPanel } from "./editor-panel";

/**
 * The editor directory's activation: installs the editor panel factory
 * and the editor commands (save, close, cycle) the shortcut bindings
 * dispatch to, plus the chunk-sourced lifecycle - the closed-editor
 * stack and the activeEditor/editorLangId context keys, both following
 * the dock. Called once by the panel registry when the directory's
 * chunk first loads; the returned disposable is held for the page
 * lifetime.
 */
export function register(): IDisposable {
  const store = new DisposableStore();
  store.add(registerPanelFactory("editor", () => new EditorPanel()));
  store.add(registerCommand("editor.save", { label: "Save", shortcut: "Ctrl+S", run: saveActiveEditor }));
  store.add(registerCommand("editor.close", { label: "Close", shortcut: "Ctrl+W", run: closeActiveEditor }));
  store.add(registerCommand("editor.cycleNext", { run: () => cycleEditor(1) }));
  store.add(registerCommand("editor.cyclePrevious", { run: () => cycleEditor(-1) }));
  const dock = getService(DOCK);
  store.add(installClosedEditorTracking(dock));
  store.add(bindEditorContextKeys(dock));
  return store;
}
