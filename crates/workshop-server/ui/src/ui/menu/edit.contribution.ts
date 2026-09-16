// The edit contribution: the six text-editing rows every text surface
// shares (plan step 18), registered eagerly at module scope. Undo,
// redo, and select-all route through the text-control service - the
// focused widget's adapter when it reports history depth, the native
// execCommand fallback otherwise; cut, copy, and paste are always the
// native path, refocused onto the remembered editable so CodeMirror
// and ProseMirror serve them through their clipboard events. Every
// row carries the textInputFocus precondition, which the action
// registry also ANDs into the keybinding rule, so the chords fire only
// while a text control holds focus. The rows are workbench-level: no
// weight, so the WorkbenchContrib default applies.
//
// The run bodies resolve the service at call time; this module pulls
// no widget code into the initial bundle.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { getService } from "../../services/service-registry";
import { TEXT_CONTROL_SERVICE } from "../../services/text-control-service";

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`edit action '${action.id}': ${result.error.message}`);
  }
}

/** One catalog row: a text-control command with its menu and chord placement. */
interface EditActionRow {
  readonly id: string;
  readonly title: string;
  readonly menu: MenuId;
  readonly group: string;
  readonly keybinding: string;
  readonly run: () => void;
}

const editActions = [
  { id: "undo", title: "Undo", menu: MenuId.MenubarEditMenu, group: "1_do", keybinding: "ctrlcmd+z", run: () => getService(TEXT_CONTROL_SERVICE).undo() },
  { id: "redo", title: "Redo", menu: MenuId.MenubarEditMenu, group: "1_do", keybinding: "ctrlcmd+y", run: () => getService(TEXT_CONTROL_SERVICE).redo() },
  { id: "editor.action.clipboardCutAction", title: "Cut", menu: MenuId.MenubarEditMenu, group: "2_ccp", keybinding: "ctrlcmd+x", run: () => getService(TEXT_CONTROL_SERVICE).execCommand("cut") },
  { id: "editor.action.clipboardCopyAction", title: "Copy", menu: MenuId.MenubarEditMenu, group: "2_ccp", keybinding: "ctrlcmd+c", run: () => getService(TEXT_CONTROL_SERVICE).execCommand("copy") },
  { id: "editor.action.clipboardPasteAction", title: "Paste", menu: MenuId.MenubarEditMenu, group: "2_ccp", keybinding: "ctrlcmd+v", run: () => getService(TEXT_CONTROL_SERVICE).execCommand("paste") },
  { id: "editor.action.selectAll", title: "Select All", menu: MenuId.MenubarSelectionMenu, group: "1_basic", keybinding: "ctrlcmd+a", run: () => getService(TEXT_CONTROL_SERVICE).selectAll() },
] satisfies readonly EditActionRow[];

for (const row of editActions) {
  addAction({
    id: row.id,
    title: row.title,
    f1: true,
    precondition: "textInputFocus",
    keybinding: { keybinding: row.keybinding },
    menu: [{ id: row.menu, group: row.group }],
    run: row.run,
  });
}
