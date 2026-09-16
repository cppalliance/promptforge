// The editor contribution: the eager module registering the editor's
// CodeMirror-backed catalog rows (plan step 13) at module scope, before
// any service exists. Every run body lazy-imports editor-commands, so
// this file pulls no CodeMirror or dockview into the initial bundle; the
// type-only `typeof import` below is erased at compile time.
//
// Placements follow the catalog: ctrl-based chords bind ctrlcmd so
// macOS gets Cmd, every keybinding rule carries when: "editorTextFocus"
// with the menu precondition "activeEditor" ANDed in by the action
// registry, and editor actions register at EditorContrib so workbench
// chords outrank them. Rows the catalog shows without a keybinding
// (Duplicate Selection, Add Previous Occurrence, Select All Occurrences)
// register none.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingWeight } from "../../services/keybinding-registry";
import { MenuId } from "../../services/menu-registry";
import { QuickAccessRegistry } from "../../services/quick-access-registry";
import { getService } from "../../services/service-registry";
import { QUICK_INPUT_SERVICE } from "../quickinput/quick-input";
import { EDITOR_SETTINGS_SERVICE, type EditorSettingName } from "./editor-settings-service";
import { createGotoLineProvider } from "./goto-line";

/** The editor-commands module as a type only; the runtime import stays lazy. */
type EditorCommands = typeof import("./editor-commands");

/** One catalog row: an editor command with its menu and keybinding placement. */
interface EditorActionRow {
  readonly id: string;
  readonly title: string;
  readonly menu: MenuId;
  readonly group: string;
  readonly keybinding?: string;
  readonly pick: (commands: EditorCommands) => Parameters<EditorCommands["runInActiveEditor"]>[0];
}

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`editor action '${action.id}': ${result.error.message}`);
  }
}

/** Builds a run body that loads the editor chunk on demand and runs one command. */
function runEditorCommand(pick: EditorActionRow["pick"]): () => Promise<void> {
  return () =>
    import("./editor-commands").then((commands) => {
      commands.runInActiveEditor(pick(commands));
    });
}

const editorActions = [
  { id: "actions.find", title: "Find", menu: MenuId.MenubarEditMenu, group: "3_find", keybinding: "ctrlcmd+f", pick: (c) => c.openSearchPanel },
  { id: "editor.action.startFindReplaceAction", title: "Replace", menu: MenuId.MenubarEditMenu, group: "3_find", keybinding: "ctrlcmd+h", pick: (c) => c.startFindReplace },
  { id: "editor.action.commentLine", title: "Toggle Line Comment", menu: MenuId.MenubarEditMenu, group: "5_insert", keybinding: "ctrlcmd+/", pick: (c) => c.toggleLineComment },
  { id: "editor.action.blockComment", title: "Toggle Block Comment", menu: MenuId.MenubarEditMenu, group: "5_insert", keybinding: "shift+alt+a", pick: (c) => c.toggleBlockComment },
  { id: "editor.action.smartSelect.expand", title: "Expand Selection", menu: MenuId.MenubarSelectionMenu, group: "1_basic", keybinding: "shift+alt+right", pick: (c) => c.smartSelectExpand },
  { id: "editor.action.smartSelect.shrink", title: "Shrink Selection", menu: MenuId.MenubarSelectionMenu, group: "1_basic", keybinding: "shift+alt+left", pick: (c) => c.smartSelectShrink },
  { id: "editor.action.copyLinesUpAction", title: "Copy Line Up", menu: MenuId.MenubarSelectionMenu, group: "2_line", keybinding: "shift+alt+up", pick: (c) => c.copyLineUp },
  { id: "editor.action.copyLinesDownAction", title: "Copy Line Down", menu: MenuId.MenubarSelectionMenu, group: "2_line", keybinding: "shift+alt+down", pick: (c) => c.copyLineDown },
  { id: "editor.action.moveLinesUpAction", title: "Move Line Up", menu: MenuId.MenubarSelectionMenu, group: "2_line", keybinding: "alt+up", pick: (c) => c.moveLineUp },
  { id: "editor.action.moveLinesDownAction", title: "Move Line Down", menu: MenuId.MenubarSelectionMenu, group: "2_line", keybinding: "alt+down", pick: (c) => c.moveLineDown },
  { id: "editor.action.duplicateSelection", title: "Duplicate Selection", menu: MenuId.MenubarSelectionMenu, group: "2_line", pick: (c) => c.duplicateSelection },
  { id: "editor.action.insertCursorAbove", title: "Add Cursor Above", menu: MenuId.MenubarSelectionMenu, group: "3_multi", keybinding: "ctrlcmd+alt+up", pick: (c) => c.insertCursorAbove },
  { id: "editor.action.insertCursorBelow", title: "Add Cursor Below", menu: MenuId.MenubarSelectionMenu, group: "3_multi", keybinding: "ctrlcmd+alt+down", pick: (c) => c.insertCursorBelow },
  { id: "editor.action.insertCursorAtEndOfEachLineSelected", title: "Add Cursors to Line Ends", menu: MenuId.MenubarSelectionMenu, group: "3_multi", keybinding: "shift+alt+i", pick: (c) => c.insertCursorAtLineEnds },
  { id: "editor.action.addSelectionToNextFindMatch", title: "Add Next Occurrence", menu: MenuId.MenubarSelectionMenu, group: "3_multi", keybinding: "ctrlcmd+d", pick: (c) => c.selectNextOccurrence },
  { id: "editor.action.addSelectionToPreviousFindMatch", title: "Add Previous Occurrence", menu: MenuId.MenubarSelectionMenu, group: "3_multi", pick: (c) => c.selectPreviousOccurrence },
  { id: "editor.action.selectHighlights", title: "Select All Occurrences", menu: MenuId.MenubarSelectionMenu, group: "3_multi", pick: (c) => c.selectSelectionMatches },
  { id: "editor.action.jumpToBracket", title: "Go to Bracket", menu: MenuId.MenubarGoMenu, group: "5_infile_nav", keybinding: "ctrlcmd+shift+\\", pick: (c) => c.cursorMatchingBracket },
  { id: "editor.action.marker.nextInFiles", title: "Next Problem", menu: MenuId.MenubarGoMenu, group: "6_problem_nav", keybinding: "f8", pick: (c) => c.nextDiagnostic },
  { id: "editor.action.marker.prevInFiles", title: "Previous Problem", menu: MenuId.MenubarGoMenu, group: "6_problem_nav", keybinding: "shift+f8", pick: (c) => c.previousDiagnostic },
] satisfies readonly EditorActionRow[];

for (const row of editorActions) {
  addAction({
    id: row.id,
    title: row.title,
    f1: true,
    precondition: "activeEditor",
    keybinding:
      row.keybinding === undefined
        ? undefined
        : { keybinding: row.keybinding, when: "editorTextFocus", weight: KeybindingWeight.EditorContrib },
    menu: [{ id: row.menu, group: row.group }],
    run: runEditorCommand(row.pick),
  });
}

/** One settings toggle row: the action flips a setting on the editor settings service. */
interface EditorToggleRow {
  readonly id: string;
  readonly title: string;
  readonly menu: MenuId;
  readonly group: string;
  readonly setting: EditorSettingName;
  readonly keybinding?: string;
  readonly precondition?: string;
}

// The four editor settings toggles (plan step 14). Each carries its
// `toggled` expression naming the config.editor.* key the settings
// service publishes, and its run body flips the setting - the surfaces
// follow through their compartments. The Appearance rows target the
// submenu id the menubar contribution declares; MenuId is a plain
// string, so the literal is the id. Word Wrap and the render toggles
// carry no precondition (they toggle global state, no editor needed);
// Column Selection Mode keeps the editor default, activeEditor.
const editorToggles = [
  { id: "editor.action.toggleColumnSelection", title: "Column Selection Mode", menu: MenuId.MenubarSelectionMenu, group: "4_config", setting: "columnSelection", precondition: "activeEditor" },
  { id: "editor.action.toggleWordWrap", title: "Word Wrap", menu: MenuId.MenubarViewMenu, group: "5_editor", setting: "wordWrap", keybinding: "alt+z" },
  { id: "editor.action.toggleRenderWhitespace", title: "Render Whitespace", menu: "menubar/view/appearance", group: "4_editor", setting: "renderWhitespace" },
  { id: "editor.action.toggleRenderControlCharacter", title: "Render Control Characters", menu: "menubar/view/appearance", group: "4_editor", setting: "renderControlCharacters" },
] satisfies readonly EditorToggleRow[];

for (const row of editorToggles) {
  addAction({
    id: row.id,
    title: row.title,
    f1: true,
    precondition: row.precondition,
    toggled: `config.editor.${row.setting}`,
    keybinding:
      row.keybinding === undefined
        ? undefined
        : { keybinding: row.keybinding, when: "editorTextFocus", weight: KeybindingWeight.EditorContrib },
    menu: [{ id: row.menu, group: row.group }],
    run: () => {
      getService(EDITOR_SETTINGS_SERVICE).toggle(row.setting);
    },
  });
}

// The lifecycle rows (plan step 15). New Text File and Reopen Closed
// Editor bind no `when`: both must work with no editor open. Go to
// Line keeps the editor-owned default - keybinding when
// editorTextFocus, menu precondition activeEditor - and opens quick
// input at the ":" prefix. The run bodies lazy-import the lifecycle
// module, so this file stays out of the editor chunk's graph.
addAction({
  id: "workbench.action.files.newUntitledFile",
  title: "New Text File",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+n", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "1_new" }],
  run: () => import("./editor-lifecycle").then((lifecycle) => lifecycle.newUntitledFile()),
});

addAction({
  id: "workbench.action.reopenClosedEditor",
  title: "Reopen Closed Editor",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+shift+t", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarRecentMenu, group: "1_editor" }],
  run: () => import("./editor-lifecycle").then((lifecycle) => lifecycle.reopenClosedEditor()),
});

addAction({
  id: "workbench.action.gotoLine",
  title: "Go to Line/Column...",
  f1: true,
  precondition: "activeEditor",
  keybinding: { keybinding: "ctrlcmd+g", when: "editorTextFocus", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarGoMenu, group: "5_infile_nav" }],
  run: () => {
    getService(QUICK_INPUT_SERVICE).quickAccess.show(":");
  },
});

// The ":" go-to-line provider. The factory is CodeMirror-free; only the
// row's accept path lazy-imports the command layer.
QuickAccessRegistry.registerQuickAccessProvider({
  prefix: ":",
  placeholder: "Go to line",
  helpEntries: [{ description: "Go to Line/Column in Editor", prefix: ":" }],
  factory: () => createGotoLineProvider(),
});
