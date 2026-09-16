// The files contribution: the eager module registering the File menu's
// picker and file-action rows (plan step 16) at module scope, before any
// service exists. Every run body lazy-imports file-actions, so this file
// pulls no dockview or CodeMirror into the initial bundle; the type-only
// import below is erased at compile time.
//
// Placements follow the catalog: ctrl-based chords bind ctrlcmd so
// macOS gets Cmd, Open File and Save As are desktop-only (precondition
// !isWeb), Save As and Revert File need an active editor, and Open
// Folder / Add Folder to Workspace share the tree's Add Folder flow.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingWeight } from "../../services/keybinding-registry";
import { MenuId } from "../../services/menu-registry";

/** The file-actions module as a type only; the runtime import stays lazy. */
type FileActions = typeof import("./file-actions");

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`files action '${action.id}': ${result.error.message}`);
  }
}

/** Builds a run body that loads the file-actions module on demand. */
function runFileAction(pick: (actions: FileActions) => () => void | Promise<void>): () => Promise<void> {
  return () => import("./file-actions").then((actions) => pick(actions)());
}

addAction({
  id: "workbench.action.files.openFile",
  title: "Open File...",
  f1: true,
  precondition: "!isWeb",
  keybinding: { keybinding: "ctrlcmd+o", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open" }],
  run: runFileAction((actions) => actions.openFile),
});

addAction({
  id: "workbench.action.files.openFolder",
  title: "Open Folder...",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+m ctrlcmd+o", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open" }],
  run: runFileAction((actions) => actions.openFolder),
});

addAction({
  id: "workbench.action.addRootFolder",
  title: "Add Folder to Workspace...",
  f1: true,
  menu: [{ id: MenuId.MenubarFileMenu, group: "3_workspace" }],
  run: runFileAction((actions) => actions.addRootFolder),
});

addAction({
  id: "workbench.action.files.saveAs",
  title: "Save As...",
  f1: true,
  precondition: "!isWeb && activeEditor",
  keybinding: { keybinding: "ctrlcmd+shift+s", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "4_save" }],
  run: runFileAction((actions) => actions.saveActiveEditorAs),
});

addAction({
  id: "workbench.action.files.saveAll",
  title: "Save All",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+m s", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "4_save" }],
  run: runFileAction((actions) => actions.saveAllEditors),
});

addAction({
  id: "workbench.action.files.revert",
  title: "Revert File",
  f1: true,
  precondition: "activeEditor",
  menu: [{ id: MenuId.MenubarFileMenu, group: "6_close" }],
  run: runFileAction((actions) => actions.revertActiveEditor),
});
