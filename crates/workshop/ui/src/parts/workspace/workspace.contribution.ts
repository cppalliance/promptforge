// The workspace contribution: the eager module registering the File
// menu's picker and file-action rows and the Open Recent and quick-open
// file providers at module scope, before any service exists. Every run
// body lazy-imports file-actions, so this file pulls no dockview or
// CodeMirror into the initial bundle; the type-only import below is
// erased at compile time.
//
// Placements follow the catalog: ctrl-based chords bind ctrlcmd so
// macOS gets Cmd, Open File and Save As are desktop-only (precondition
// !isWeb), Save As and Revert File need an active editor, and Open
// Folder / Add Folder to Workspace share the tree's Add Folder flow.
// vscode.open and vscode.openFolder are the provider-backed commands:
// the Open Recent dynamic rows and the "" quick-access rows set their
// own titles and pass the path as args[0], which the run bodies narrow
// to string, never cast. Neither is f1 - a palette row cannot supply a
// path argument, and Go to File... already owns that surface.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingWeight } from "../../services/keybinding-registry";
import { MenuId, Menus } from "../../services/menu-registry";
import { QuickAccessRegistry } from "../../services/quick-access-registry";
import { RECENT_FILES_STORE } from "../../services/recent-files-store";
import { getService } from "../../services/service-registry";
import { QUICK_INPUT_SERVICE } from "../../services/quick-input-service";
import { createFileQuickAccessProvider, createRecentMenuProvider } from "./open-recent";

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

/**
 * Builds a run body that narrows args[0] to a path string - never a
 * cast - and loads the file-actions module on demand. A missing or
 * non-string argument is a no-op.
 */
function runPathAction(
  pick: (actions: FileActions) => (path: string) => void | Promise<void>,
): (...args: readonly unknown[]) => Promise<void> {
  return (...args) => {
    const path = args[0];
    if (typeof path !== "string") {
      return Promise.resolve();
    }
    return import("./file-actions").then((actions) => pick(actions)(path));
  };
}

addAction({
  id: "workbench.action.files.openFile",
  title: "Open File...",
  f1: true,
  precondition: "!isWeb",
  keybinding: { keybinding: "ctrlcmd+o", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open", order: 1 }],
  run: runFileAction((actions) => actions.openFile),
});

addAction({
  id: "workbench.action.files.openFolder",
  title: "Open Folder...",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+m ctrlcmd+o", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open", order: 2 }],
  run: runFileAction((actions) => actions.openFolder),
});

addAction({
  id: "workbench.action.addRootFolder",
  title: "Add Folder to Workspace...",
  f1: true,
  menu: [{ id: MenuId.MenubarFileMenu, group: "3_workspace", order: 1 }],
  run: runFileAction((actions) => actions.addRootFolder),
});

addAction({
  id: "workbench.action.files.saveAs",
  title: "Save As...",
  f1: true,
  precondition: "!isWeb && activeEditor",
  keybinding: { keybinding: "ctrlcmd+shift+s", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "4_save", order: 2 }],
  run: runFileAction((actions) => actions.saveActiveEditorAs),
});

addAction({
  id: "workbench.action.files.saveAll",
  title: "Save All",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+m s", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "4_save", order: 3 }],
  run: runFileAction((actions) => actions.saveAllEditors),
});

addAction({
  id: "workbench.action.files.revert",
  title: "Revert File",
  f1: true,
  precondition: "activeEditor",
  menu: [{ id: MenuId.MenubarFileMenu, group: "6_close", order: 1 }],
  run: runFileAction((actions) => actions.revertActiveEditor),
});

// Open Recent: the two path commands its dynamic rows and the ""
// quick-access rows dispatch, then its More... and Clear rows.
addAction({
  id: "vscode.open",
  title: "Open File",
  run: runPathAction((actions) => actions.openPath),
});

addAction({
  id: "vscode.openFolder",
  title: "Open Folder",
  run: runPathAction((actions) => actions.focusWorkspaceRoot),
});

addAction({
  id: "workbench.action.openRecent",
  title: "More...",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+r", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarRecentMenu, group: "y_more" }],
  run: () => {
    getService(QUICK_INPUT_SERVICE).quickAccess.show("");
  },
});

addAction({
  id: "workbench.action.clearRecentFiles",
  title: "Clear Recently Opened...",
  f1: true,
  menu: [{ id: MenuId.MenubarRecentMenu, group: "z_clear" }],
  run: () => {
    getService(RECENT_FILES_STORE).clear();
  },
});

// The dynamic halves: Open Recent's root and recent-file rows, re-read
// at every open, and the "" quick-access provider over the same stores.
Menus.setProvider(MenuId.MenubarRecentMenu, createRecentMenuProvider());
QuickAccessRegistry.registerQuickAccessProvider({
  prefix: "",
  placeholder: "Search files by name",
  helpEntries: [{ description: "Go to File", prefix: "" }],
  factory: () => createFileQuickAccessProvider(),
});
