// Unit test for the editor commands catalog (plan step 13,
// src/ui/editor/editor-commands.ts and editor.contribution.ts) and the
// editor lifecycle (plan step 15: untitled buffers, the closed-editor
// stack, the CodeMirror text-control adapter, the ":" go-to-line
// provider, recent-files recording, and the activeEditor/editorLangId
// context keys). Every
// CodeMirror-backed catalog row is driven against a real EditorState -
// comment toggles, line copy/move, duplicate selection, cursor add rows,
// occurrence rows, bracket jump - or against a real EditorView in jsdom
// for the view commands (find, replace, smart select with its selection
// stack, diagnostic navigation). runInActiveEditor, withActiveEditor,
// and splitActiveEditor run against a stub dock, and the contribution's
// registry wiring (menu
// rows, palette rows, keybinding rules, the lazy run path) is asserted
// on the shared registries. Bundles the modules with esbuild and drives
// them in jsdom with the same measurement shims as editor-idioms.mjs.
// Run: node --test test/editor-commands.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/ui/editor/editor.contribution.ts";
      export * as editorCommands from "./src/ui/editor/editor-commands.ts";
      export * as editorLifecycle from "./src/ui/editor/editor-lifecycle.ts";
      export { parseLineColumn, createGotoLineProvider } from "./src/ui/editor/goto-line.ts";
      export { EditorState, EditorSelection } from "@codemirror/state";
      export { EditorView } from "@codemirror/view";
      export { javascript } from "@codemirror/lang-javascript";
      export { ensureSyntaxTree } from "@codemirror/language";
      export { setDiagnostics } from "@codemirror/lint";
      export { registerService, getService } from "./src/services/service-registry.ts";
      export { DOCK } from "./src/services/panel-registry.ts";
      export { EditorPanel } from "./src/ui/editor/editor-panel.ts";
      export { CodeMirrorSurface } from "./src/ui/editor/editor-surface.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus, MenuId } from "./src/services/menu-registry.ts";
      export { KeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { QuickAccessRegistry } from "./src/services/quick-access-registry.ts";
      export { CONTEXT_KEY_SERVICE } from "./src/services/context-key-service.ts";
      export { RECENT_FILES_STORE } from "./src/services/recent-files-store.ts";
      export { TEXT_CONTROL_SERVICE } from "./src/services/text-control-service.ts";
      export { initZones } from "./src/ui/layout/zones.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The modules under test import colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;

// CodeMirror measures text through Range, which jsdom does not layout;
// zero-rect shims are enough because the test never asserts geometry.
const zeroRect = () => ({
  x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0,
  toJSON: () => ({}),
});
window.Range.prototype.getBoundingClientRect = zeroRect;
window.Range.prototype.getClientRects = () => ({
  length: 0,
  item: () => null,
  [Symbol.iterator]: [][Symbol.iterator],
});
window.HTMLElement.prototype.getClientRects = function getClientRects() {
  return { length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] };
};
if (!window.HTMLElement.prototype.getBoundingClientRect) {
  window.HTMLElement.prototype.getBoundingClientRect = zeroRect;
}
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

for (const key of [
  "document",
  "navigator",
  "Window",
  "HTMLElement",
  "HTMLInputElement",
  "HTMLTextAreaElement",
  "Node",
  "Element",
  "Range",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;

// The contribution registers at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-commands-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  editorCommands,
  editorLifecycle,
  parseLineColumn,
  createGotoLineProvider,
  EditorState,
  EditorSelection,
  EditorView,
  javascript,
  ensureSyntaxTree,
  setDiagnostics,
  registerService,
  getService,
  DOCK,
  EditorPanel,
  CodeMirrorSurface,
  Commands,
  Menus,
  MenuId,
  KeybindingsRegistry,
  QuickAccessRegistry,
  CONTEXT_KEY_SERVICE,
  RECENT_FILES_STORE,
  TEXT_CONTROL_SERVICE,
  initZones,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

/** Runs a StateCommand against a real EditorState, capturing the dispatch. */
function runState(command, doc, selection, extensions = []) {
  // allowMultipleSelections mirrors the surface's basicSetup: without it
  // the state normalizes added cursors away.
  const state = EditorState.create({
    doc,
    selection,
    extensions: [EditorState.allowMultipleSelections.of(true), ...extensions],
  });
  let next = null;
  const result = command({ state, dispatch: (tr) => { next = tr.state; } });
  return { result, state: next ?? state };
}

/** Builds a real EditorView attached to the jsdom document. */
function makeView(doc, selection, extensions = []) {
  const parent = window.document.createElement("div");
  window.document.body.appendChild(parent);
  return new EditorView({
    state: EditorState.create({
      doc,
      selection,
      extensions: [EditorState.allowMultipleSelections.of(true), ...extensions],
    }),
    parent,
  });
}

const selJson = (view) => JSON.stringify(view.state.selection.ranges.map((r) => [r.from, r.to]));

{
  // Comment toggles against a real javascript state.
  const commented = runState(editorCommands.toggleLineComment, "let a = 1;\n", EditorSelection.single(4), [javascript()]);
  check("toggleLineComment comments the cursor line", commented.result === true && commented.state.doc.toString() === "// let a = 1;\n");
  const restored = runState(editorCommands.toggleLineComment, "// let a = 1;\n", EditorSelection.single(5), [javascript()]);
  check("toggleLineComment uncomments a commented line", restored.result === true && restored.state.doc.toString() === "let a = 1;\n");
  const block = runState(editorCommands.toggleBlockComment, "let a = 1;\n", EditorSelection.range(4, 5), [javascript()]);
  check("toggleBlockComment wraps the selection", block.result === true && block.state.doc.toString() === "let /* a */ = 1;\n");
}

{
  // Line copy and move.
  const copyDown = runState(editorCommands.copyLineDown, "one\ntwo\n", EditorSelection.single(1));
  check("copyLineDown duplicates the line below", copyDown.result === true && copyDown.state.doc.toString() === "one\none\ntwo\n");
  const copyUp = runState(editorCommands.copyLineUp, "one\ntwo\n", EditorSelection.single(5));
  check("copyLineUp duplicates the line above", copyUp.result === true && copyUp.state.doc.toString() === "one\ntwo\ntwo\n");
  const moveDown = runState(editorCommands.moveLineDown, "one\ntwo\n", EditorSelection.single(1));
  check("moveLineDown swaps the line down", moveDown.result === true && moveDown.state.doc.toString() === "two\none\n");
  const moveUp = runState(editorCommands.moveLineUp, "one\ntwo\n", EditorSelection.single(5));
  check("moveLineUp swaps the line up", moveUp.result === true && moveUp.state.doc.toString() === "two\none\n");
}

{
  // Duplicate Selection: empty ranges copy the line down; non-empty
  // ranges insert the selected text after each range and select the copy.
  const empty = runState(editorCommands.duplicateSelection, "one\ntwo\n", EditorSelection.single(1));
  check("duplicateSelection on an empty range copies the line down", empty.result === true && empty.state.doc.toString() === "one\none\ntwo\n");
  const single = runState(editorCommands.duplicateSelection, "ab cd", EditorSelection.range(0, 2));
  check("duplicateSelection inserts the selected text after the range", single.result === true && single.state.doc.toString() === "abab cd");
  check("duplicateSelection selects the inserted copy", single.state.selection.main.from === 2 && single.state.selection.main.to === 4);
  const multi = runState(
    editorCommands.duplicateSelection,
    "ab cd",
    EditorSelection.create([EditorSelection.range(0, 2), EditorSelection.range(3, 5)]),
  );
  check("duplicateSelection duplicates every range", multi.state.doc.toString() === "abab cdcd");
  check(
    "duplicateSelection selects both copies at their mapped positions",
    multi.state.selection.ranges.length === 2 &&
      multi.state.selection.ranges[0].from === 2 && multi.state.selection.ranges[0].to === 4 &&
      multi.state.selection.ranges[1].from === 7 && multi.state.selection.ranges[1].to === 9,
  );
}

{
  // Add Cursor Above / Below: one new cursor per head, same column,
  // clamped to the target line's end; a no-op past the document edge.
  const below = runState(editorCommands.insertCursorBelow, "ab\nxy\n", EditorSelection.cursor(1));
  check(
    "insertCursorBelow adds a cursor one line down at the same column",
    below.result === true && below.state.selection.ranges.length === 2 && below.state.selection.ranges[1].head === 4,
  );
  const clamped = runState(editorCommands.insertCursorBelow, "abc\nx\n", EditorSelection.cursor(2));
  check("insertCursorBelow clamps the column to a shorter line", clamped.state.selection.ranges[1].head === 5);
  const above = runState(editorCommands.insertCursorAbove, "ab\nxy\n", EditorSelection.cursor(4));
  check(
    "insertCursorAbove adds a cursor one line up",
    above.result === true && above.state.selection.ranges.length === 2 && above.state.selection.ranges[0].head === 1,
  );
  const topEdge = runState(editorCommands.insertCursorAbove, "ab\nxy\n", EditorSelection.cursor(1));
  check("insertCursorAbove on the first line is a no-op", topEdge.result === false && topEdge.state.selection.main.head === 1);
  const bottomEdge = runState(editorCommands.insertCursorBelow, "ab\nxy", EditorSelection.cursor(4));
  check("insertCursorBelow on the last line is a no-op", bottomEdge.result === false);
}

{
  // Add Cursors to Line Ends: one cursor per touched line; a selection
  // ending exactly at a line start does not touch that line.
  const ends = runState(editorCommands.insertCursorAtLineEnds, "aa\nbb\ncc", EditorSelection.range(0, 4));
  check(
    "insertCursorAtLineEnds adds a cursor at every touched line's end",
    ends.result === true &&
      ends.state.selection.ranges.length === 2 &&
      ends.state.selection.ranges[0].head === 2 &&
      ends.state.selection.ranges[1].head === 5,
  );
  const trailing = runState(editorCommands.insertCursorAtLineEnds, "aa\nbb\ncc", EditorSelection.range(0, 3));
  check(
    "a selection ending at a line start does not touch that line",
    trailing.state.selection.ranges.length === 1 && trailing.state.selection.ranges[0].head === 2,
  );
}

{
  // Occurrence rows: next, previous (custom, backwards and wrapping),
  // and select-all.
  const next = runState(editorCommands.selectNextOccurrence, "foo bar foo", EditorSelection.range(0, 3));
  check(
    "selectNextOccurrence adds the next match",
    next.result === true && next.state.selection.ranges.length === 2 &&
      next.state.selection.ranges[1].from === 8 && next.state.selection.ranges[1].to === 11,
  );
  const previous = runState(editorCommands.selectPreviousOccurrence, "foo bar foo", EditorSelection.range(8, 11));
  check(
    "selectPreviousOccurrence adds the previous match",
    previous.result === true && previous.state.selection.ranges.length === 2 &&
      previous.state.selection.ranges[0].from === 0 && previous.state.selection.ranges[0].to === 3,
  );
  const wraps = runState(editorCommands.selectPreviousOccurrence, "foo bar foo", EditorSelection.range(0, 3));
  check(
    "selectPreviousOccurrence wraps to the last match",
    wraps.result === true && wraps.state.selection.ranges.length === 2 && wraps.state.selection.ranges[1].from === 8,
  );
  const emptySel = runState(editorCommands.selectPreviousOccurrence, "foo bar foo", EditorSelection.cursor(4));
  check("selectPreviousOccurrence on an empty selection is a no-op", emptySel.result === false);
  const only = runState(editorCommands.selectPreviousOccurrence, "foo bar", EditorSelection.range(0, 3));
  check("selectPreviousOccurrence with only the selected match is a no-op", only.result === false);
  const all = runState(editorCommands.selectSelectionMatches, "foo bar foo", EditorSelection.range(0, 3));
  check("selectSelectionMatches selects every match", all.result === true && all.state.selection.ranges.length === 2);
}

{
  // Go to Bracket.
  const jump = runState(editorCommands.cursorMatchingBracket, "(ab)", EditorSelection.cursor(1));
  check("cursorMatchingBracket jumps to the matching bracket", jump.result === true && jump.state.selection.main.head === 3);
}

{
  // Find and Replace open the search panel; Replace focuses the
  // replace field.
  const findView = makeView("alpha\nbeta\n", EditorSelection.cursor(0));
  check("openSearchPanel opens the search panel", editorCommands.openSearchPanel(findView) === true);
  check("the search panel is in the editor DOM", findView.dom.querySelector(".cm-panel") !== null);
  findView.destroy();

  const replaceView = makeView("alpha\nbeta\n", EditorSelection.cursor(0));
  check("startFindReplace opens the panel", editorCommands.startFindReplace(replaceView) === true);
  const replaceInput = replaceView.dom.querySelector("input[name=replace]");
  check(
    "startFindReplace focuses the replace field",
    replaceInput !== null && window.document.activeElement === replaceInput,
  );
  replaceView.destroy();
}

{
  // Smart select: expand pushes the pre-expansion selection, shrink
  // pops and restores it, and an empty stack is a no-op.
  const view = makeView("const x = foo(1, 2);\n", EditorSelection.cursor(15), [javascript()]);
  ensureSyntaxTree(view.state, view.state.doc.length);
  const original = selJson(view);
  check(
    "smartSelectExpand grows the cursor into a node selection",
    editorCommands.smartSelectExpand(view) === true && !view.state.selection.main.empty,
  );
  const expanded1 = selJson(view);
  check("smartSelectExpand grows the selection again", editorCommands.smartSelectExpand(view) === true && selJson(view) !== expanded1);
  const expanded2 = selJson(view);
  check("the second expansion differs from the first", expanded2 !== expanded1);
  check("smartSelectShrink restores the previous expansion", editorCommands.smartSelectShrink(view) === true && selJson(view) === expanded1);
  check("smartSelectShrink restores the original cursor", editorCommands.smartSelectShrink(view) === true && selJson(view) === original);
  check("smartSelectShrink on an empty stack is a no-op", editorCommands.smartSelectShrink(view) === false);
  view.destroy();

  const plain = makeView("plain text\n", EditorSelection.cursor(2));
  check("smartSelectExpand without a syntax tree returns false", editorCommands.smartSelectExpand(plain) === false);
  check("a failed expand pushes nothing to shrink", editorCommands.smartSelectShrink(plain) === false);
  plain.destroy();
}

{
  // Diagnostic navigation over setDiagnostics.
  const view = makeView("alpha\nbeta\ngamma\n", EditorSelection.cursor(5));
  view.dispatch(
    setDiagnostics(view.state, [
      { from: 0, to: 2, severity: "error", message: "one" },
      { from: 11, to: 14, severity: "warning", message: "two" },
    ]),
  );
  check(
    "nextDiagnostic selects the next diagnostic",
    editorCommands.nextDiagnostic(view) === true &&
      view.state.selection.main.from === 11 && view.state.selection.main.head === 14,
  );
  check(
    "previousDiagnostic selects the previous diagnostic",
    editorCommands.previousDiagnostic(view) === true &&
      view.state.selection.main.from === 0 && view.state.selection.main.head === 2,
  );
  view.destroy();
  const clean = makeView("alpha\n", EditorSelection.cursor(0));
  check("nextDiagnostic with no lint state is a no-op", editorCommands.nextDiagnostic(clean) === false);
  clean.destroy();
}

{
  // runInActiveEditor and withActiveEditor against a stub dock.
  const unregisterEmpty = registerService(DOCK, () => ({ activePanel: undefined, panels: [] }));
  check(
    "runInActiveEditor without an active editor returns false",
    editorCommands.runInActiveEditor(editorCommands.copyLineDown) === false,
  );
  let called = 0;
  editorCommands.withActiveEditor(() => { called += 1; });
  check("withActiveEditor without an active editor does not call fn", called === 0);
  unregisterEmpty.dispose();

  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\a.txt", text: "one\ntwo\n" });
  const panel = new EditorPanel({ createSurface: () => surface });
  const fakePanel = { view: { content: panel } };
  const unregisterDock = registerService(DOCK, () => ({ activePanel: fakePanel, panels: [fakePanel] }));
  check(
    "runInActiveEditor runs the command against the active view",
    editorCommands.runInActiveEditor(editorCommands.copyLineDown) === true,
  );
  check("the command changed the active editor's document", surface.text() === "one\none\ntwo\n");
  let seen = null;
  editorCommands.withActiveEditor((p) => { seen = p; });
  check("withActiveEditor passes the active editor panel", seen === panel);
  unregisterDock.dispose();
  panel.dispose();
}

{
  // splitActiveEditor against a fake dock recording addGroup/moveTo:
  // each Split direction maps to its dockview direction, the active
  // editor moves into the fresh group, and a missing or non-editor
  // active panel is a no-op.
  const splitSurface = new CodeMirrorSurface();
  window.document.body.appendChild(splitSurface.element);
  splitSurface.open({ path: "C:\\project\\split.txt", text: "one\n" });
  const splitPanel = new EditorPanel({ createSurface: () => splitSurface });
  const moveTos = [];
  const splitDockPanel = {
    view: { content: splitPanel },
    api: { moveTo: (target) => moveTos.push(target) },
  };
  const addGroups = [];
  const group = { id: "split-group" };
  const unregisterSplitDock = registerService(DOCK, () => ({
    activePanel: splitDockPanel,
    panels: [splitDockPanel],
    addGroup: (options) => {
      addGroups.push(options);
      return group;
    },
  }));

  const directions = { up: "above", down: "below", left: "left", right: "right" };
  for (const [split, dockviewDirection] of Object.entries(directions)) {
    addGroups.length = 0;
    moveTos.length = 0;
    editorCommands.splitActiveEditor(split);
    check(
      `split ${split} adds a group ${dockviewDirection} of the active editor`,
      addGroups.length === 1 &&
        addGroups[0].referencePanel === splitDockPanel &&
        addGroups[0].direction === dockviewDirection,
    );
    check(
      `split ${split} moves the active editor into the new group`,
      moveTos.length === 1 && moveTos[0].group === group,
    );
  }
  unregisterSplitDock.dispose();

  // No active editor: addGroup is never called.
  const emptyAddGroups = [];
  const unregisterEmptySplit = registerService(DOCK, () => ({
    activePanel: undefined,
    panels: [],
    addGroup: (options) => {
      emptyAddGroups.push(options);
      return group;
    },
  }));
  editorCommands.splitActiveEditor("right");
  check("split with no active editor is a no-op", emptyAddGroups.length === 0);
  unregisterEmptySplit.dispose();

  // A non-editor active panel is a no-op too.
  const foreignAddGroups = [];
  const unregisterForeignSplit = registerService(DOCK, () => ({
    activePanel: { view: { content: {} }, api: {} },
    panels: [],
    addGroup: (options) => {
      foreignAddGroups.push(options);
      return group;
    },
  }));
  editorCommands.splitActiveEditor("right");
  check("split with a non-editor active panel is a no-op", foreignAddGroups.length === 0);
  unregisterForeignSplit.dispose();
  splitPanel.dispose();
}

{
  // The contribution's registry wiring: every catalog row lands in the
  // command registry, its menu, and the palette; keybound rows get a
  // label, the three keybinding-less rows get none.
  const EXPECTED_IDS = [
    "actions.find",
    "editor.action.startFindReplaceAction",
    "editor.action.commentLine",
    "editor.action.blockComment",
    "editor.action.smartSelect.expand",
    "editor.action.smartSelect.shrink",
    "editor.action.copyLinesUpAction",
    "editor.action.copyLinesDownAction",
    "editor.action.moveLinesUpAction",
    "editor.action.moveLinesDownAction",
    "editor.action.duplicateSelection",
    "editor.action.insertCursorAbove",
    "editor.action.insertCursorBelow",
    "editor.action.insertCursorAtEndOfEachLineSelected",
    "editor.action.addSelectionToNextFindMatch",
    "editor.action.addSelectionToPreviousFindMatch",
    "editor.action.selectHighlights",
    "editor.action.jumpToBracket",
    "editor.action.marker.nextInFiles",
    "editor.action.marker.prevInFiles",
    "workbench.action.files.newUntitledFile",
    "workbench.action.reopenClosedEditor",
    "workbench.action.gotoLine",
    // Step 20: the workbench-level editor rows registered when the menu
    // tree assembled - Save, Close Editor, the four splits, and cycling.
    "workbench.action.files.save",
    "workbench.action.closeActiveEditor",
    "workbench.action.splitEditorUp",
    "workbench.action.splitEditorDown",
    "workbench.action.splitEditorLeft",
    "workbench.action.splitEditorRight",
    "workbench.action.nextEditor",
    "workbench.action.previousEditor",
  ];
  check("the contribution registered without errors", consoleErrors.length === 0);
  check(
    "every catalog row is a registered command",
    EXPECTED_IDS.every((id) => Commands.lookup(id) !== undefined),
  );
  const palette = Menus.getMenuItems(MenuId.CommandPalette).map((row) => row.command);
  // The catalog rows plus step 14's four settings toggles; EXPECTED_IDS
  // includes step 15's three lifecycle actions.
  check(
    "every catalog row reaches the palette",
    palette.length === EXPECTED_IDS.length + 4 && EXPECTED_IDS.every((id) => palette.includes(id)),
  );
  check("find carries the activeEditor precondition", Commands.lookup("actions.find")?.precondition === "activeEditor");
  check("commentLine keeps its catalog title", Commands.lookup("editor.action.commentLine")?.title === "Toggle Line Comment");

  const selectionMenu = Menus.getMenuItems(MenuId.MenubarSelectionMenu).map((row) => row.command);
  // The thirteen catalog rows plus step 14's Column Selection Mode toggle.
  check(
    "the Selection menu carries its thirteen rows",
    selectionMenu.length === 14 &&
      ["editor.action.smartSelect.expand", "editor.action.smartSelect.shrink"].every((id) => selectionMenu.includes(id)),
  );
  const editMenu = Menus.getMenuItems(MenuId.MenubarEditMenu).map((row) => row.command);
  check(
    "the Edit menu carries find, replace, and the comment toggles",
    editMenu.length === 4 &&
      ["actions.find", "editor.action.startFindReplaceAction", "editor.action.commentLine", "editor.action.blockComment"]
        .every((id) => editMenu.includes(id)),
  );
  const goMenu = Menus.getMenuItems(MenuId.MenubarGoMenu).map((row) => row.command);
  check(
    "the Go menu carries bracket jump, problem navigation, and go-to-line",
    goMenu.length === 4 &&
      ["editor.action.jumpToBracket", "editor.action.marker.nextInFiles", "editor.action.marker.prevInFiles", "workbench.action.gotoLine"]
        .every((id) => goMenu.includes(id)),
  );
  const fileMenu = Menus.getMenuItems(MenuId.MenubarFileMenu).map((row) => row.command);
  // Updated in step 20: the editor contribution also registers Save and
  // Close Editor (File 4_save / 6_close) when the menu tree assembled.
  check(
    "the File menu carries New Text File, Save, and Close Editor",
    fileMenu.length === 3 &&
      ["workbench.action.files.newUntitledFile", "workbench.action.files.save", "workbench.action.closeActiveEditor"].every((id) =>
        fileMenu.includes(id),
      ),
  );
  const recentMenu = Menus.getMenuItems(MenuId.MenubarRecentMenu).map((row) => row.command);
  check(
    "Open Recent carries Reopen Closed Editor",
    recentMenu.length === 1 && recentMenu.includes("workbench.action.reopenClosedEditor"),
  );
  check("Go to Line carries the activeEditor precondition", Commands.lookup("workbench.action.gotoLine")?.precondition === "activeEditor");
  check("New Text File has a keybinding label", KeybindingsRegistry.lookupKeybinding("workbench.action.files.newUntitledFile") !== undefined);
  check("Reopen Closed Editor has a keybinding label", KeybindingsRegistry.lookupKeybinding("workbench.action.reopenClosedEditor") !== undefined);
  check("Go to Line has a keybinding label", KeybindingsRegistry.lookupKeybinding("workbench.action.gotoLine") !== undefined);
  check("the contribution registers the ':' go-to-line provider", QuickAccessRegistry.getQuickAccessProvider(":12")?.prefix === ":");

  check("a keybound row gets a keybinding label", KeybindingsRegistry.lookupKeybinding("editor.action.commentLine") !== undefined);
  check("Duplicate Selection has no keybinding", KeybindingsRegistry.lookupKeybinding("editor.action.duplicateSelection") === undefined);
  check(
    "Add Previous Occurrence has no keybinding",
    KeybindingsRegistry.lookupKeybinding("editor.action.addSelectionToPreviousFindMatch") === undefined,
  );
  check("Select All Occurrences has no keybinding", KeybindingsRegistry.lookupKeybinding("editor.action.selectHighlights") === undefined);

  // Executing through the command registry exercises the run body's
  // lazy import of editor-commands.
  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\b.txt", text: "alpha\nbeta\n" });
  const panel = new EditorPanel({ createSurface: () => surface });
  const fakePanel = { view: { content: panel } };
  const unregisterDock = registerService(DOCK, () => ({ activePanel: fakePanel, panels: [fakePanel] }));
  const executed = await Commands.execute("editor.action.copyLinesDownAction");
  check(
    "executing a catalog command runs through the lazy import",
    executed === true && surface.text() === "alpha\nalpha\nbeta\n",
  );
  unregisterDock.dispose();
  panel.dispose();
}

// --- Step 15: editor lifecycle --------------------------------------------
// A stub surface with the EditorSurface contract's dirty semantics, for
// panel-level tests that never touch CodeMirror.
function createStubSurface() {
  const listeners = new Set();
  return {
    element: window.document.createElement("div"),
    currentText: "",
    dirty: false,
    opened: [],
    open(document) {
      this.opened.push(document);
      this.currentText = document.text;
      this.setDirty(false);
    },
    text() {
      return this.currentText;
    },
    markSaved(text) {
      this.setDirty(this.currentText !== text);
    },
    isDirty() {
      return this.dirty;
    },
    setReadOnly() {},
    onDirtyChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    editorView() {
      return null;
    },
    focus() {},
    dispose() {},
    setDirty(dirty) {
      if (dirty === this.dirty) return;
      this.dirty = dirty;
      for (const listener of listeners) listener(dirty);
    },
  };
}

// A fake dock good enough for zones and the lifecycle: panel opens are
// recorded, and the add/remove/active listeners are test-driven.
function makeFakeDock() {
  const listeners = { add: [], remove: [], active: [], move: [] };
  const added = [];
  const dock = {
    panels: [],
    groups: [],
    activePanel: undefined,
    onDidMovePanel: (fn) => {
      listeners.move.push(fn);
      return { dispose() {} };
    },
    onDidAddPanel: (fn) => {
      listeners.add.push(fn);
      return { dispose() {} };
    },
    onDidRemovePanel: (fn) => {
      listeners.remove.push(fn);
      return { dispose() {} };
    },
    onDidActivePanelChange: (fn) => {
      listeners.active.push(fn);
      return { dispose() {} };
    },
    getPanel: () => undefined,
    getGroup: () => undefined,
    addPanel: (options) => {
      added.push(options);
      const panel = {
        id: options.id,
        params: options.params,
        group: { id: "g1" },
        api: { setActive() {} },
        view: { content: {} },
      };
      dock.panels.push(panel);
      return panel;
    },
  };
  return { dock, listeners, added };
}

const { dock, listeners, added } = makeFakeDock();
initZones(dock);

{
  // Untitled buffers: the panel titles itself, reads nothing, and its
  // save delegates to the Save As command.
  const recentsBefore = getService(RECENT_FILES_STORE).list.length;
  let reads = 0;
  const untitledStub = createStubSurface();
  const untitledPanel = new EditorPanel({
    createSurface: () => untitledStub,
    readFile: async () => {
      reads += 1;
      throw new Error("an untitled buffer must not read");
    },
  });
  const untitledTitles = [];
  untitledPanel.init({
    params: { untitled: 7 },
    api: { setTitle: (title) => untitledTitles.push(title), close() {} },
  });
  check("an untitled panel titles itself Untitled-N", untitledTitles.at(-1) === "Untitled-7");
  check(
    "an untitled panel opens an empty buffer without reading",
    reads === 0 && untitledStub.opened.length === 1 && untitledStub.opened[0].text === "",
  );
  check("a fresh untitled buffer is not dirty", !untitledPanel.isDirty());
  check("an untitled buffer reports no file path", untitledPanel.filePath() === null && untitledPanel.isUntitled());
  check(
    "an untitled buffer records no recent-files entry",
    getService(RECENT_FILES_STORE).list.length === recentsBefore,
  );

  let saveAsCalls = 0;
  const saveAsRegistration = Commands.register("workbench.action.files.saveAs", {
    run: () => {
      saveAsCalls += 1;
    },
  });
  await untitledPanel.save();
  check("save on an untitled buffer runs Save As", saveAsCalls === 1);
  saveAsRegistration.dispose();
  untitledPanel.dispose();

  // A reopened untitled buffer restores its text, dirty against the
  // empty baseline: the content was never persisted.
  const restoredStub = createStubSurface();
  const restoredPanel = new EditorPanel({ createSurface: () => restoredStub });
  const restoredTitles = [];
  restoredPanel.init({
    params: { untitled: 8, text: "draft" },
    api: { setTitle: (title) => restoredTitles.push(title), close() {} },
  });
  check("a reopened untitled buffer keeps its text", restoredStub.text() === "draft");
  check("a reopened untitled buffer with content is dirty", restoredPanel.isDirty());
  check("a dirty untitled title carries the dot", restoredTitles.at(-1) === "● Untitled-8");
  restoredPanel.dispose();
}

{
  // newUntitledFile opens an untitled editor panel per call.
  editorLifecycle.newUntitledFile();
  editorLifecycle.newUntitledFile();
  check(
    "newUntitledFile opens an untitled editor panel",
    added.length === 2 && typeof added[0].params.untitled === "number",
  );
  check(
    "untitled panel ids are unique per buffer",
    added[0].id !== added[1].id && added[0].id.startsWith("editor:untitled-"),
  );
  const beforeCommand = added.length;
  const executed = await Commands.execute("workbench.action.files.newUntitledFile");
  check(
    "executing New Text File runs through the lazy lifecycle import",
    executed === true && added.length === beforeCommand + 1,
  );
}

{
  // The closed-editor stack: file editors reopen by path, untitled
  // buffers reopen with their text under a fresh serial.
  const tracking = editorLifecycle.installClosedEditorTracking(dock);

  const fileStub = createStubSurface();
  const filePanel = new EditorPanel({
    createSurface: () => fileStub,
    readFile: async () => ({ path: "C:\\p\\x.txt", size: 1, token: "t1", text: "x" }),
  });
  filePanel.init({ params: { path: "C:\\p\\x.txt" }, api: { setTitle() {}, close() {} } });
  await flush();
  for (const listener of listeners.remove) listener({ id: "editor:C:\\p\\x.txt", view: { content: filePanel } });
  const beforeFileReopen = added.length;
  editorLifecycle.reopenClosedEditor();
  check(
    "reopenClosedEditor reopens the last closed file editor by path",
    added.length === beforeFileReopen + 1 && added.at(-1).params.path === "C:\\p\\x.txt",
  );

  const untitledStub = createStubSurface();
  const untitledPanel = new EditorPanel({ createSurface: () => untitledStub });
  untitledPanel.init({ params: { untitled: 3, text: "unsaved draft" }, api: { setTitle() {}, close() {} } });
  for (const listener of listeners.remove) listener({ id: "editor:untitled-3", view: { content: untitledPanel } });
  editorLifecycle.reopenClosedEditor();
  check(
    "a closed untitled editor reopens with its text under a fresh serial",
    added.at(-1).params.text === "unsaved draft" &&
      typeof added.at(-1).params.untitled === "number" &&
      added.at(-1).params.untitled !== 3,
  );

  const beforeEmpty = added.length;
  editorLifecycle.reopenClosedEditor();
  check("reopenClosedEditor on an empty stack is a no-op", added.length === beforeEmpty);

  // A non-editor panel close records nothing.
  for (const listener of listeners.remove) listener({ id: "tree", view: { content: {} } });
  editorLifecycle.reopenClosedEditor();
  check("closing a non-editor panel pushes nothing onto the stack", added.length === beforeEmpty);
  tracking.dispose();
  filePanel.dispose();
  untitledPanel.dispose();
}

{
  // The editor-sourced context keys follow the dock's active panel.
  const binding = editorLifecycle.bindEditorContextKeys(dock);
  const context = getService(CONTEXT_KEY_SERVICE);

  const markdownStub = createStubSurface();
  const markdownPanel = new EditorPanel({
    createSurface: () => markdownStub,
    readFile: async () => ({ path: "C:\\p\\notes.md", size: 4, token: "t1", text: "# hi" }),
  });
  markdownPanel.init({ params: { path: "C:\\p\\notes.md" }, api: { setTitle() {}, close() {} } });
  await flush();
  check(
    "opening a file records it in the recent-files store",
    getService(RECENT_FILES_STORE).list.includes("C:\\p\\notes.md"),
  );

  const markdownDockPanel = { id: "editor:C:\\p\\notes.md", view: { content: markdownPanel } };
  dock.activePanel = markdownDockPanel;
  for (const listener of listeners.active) listener({ panel: markdownDockPanel });
  check(
    "activeEditor is the active editor panel's id",
    context.getValue("activeEditor") === "editor:C:\\p\\notes.md",
  );
  check("editorLangId comes from the active editor's language", context.getValue("editorLangId") === "markdown");

  dock.activePanel = { id: "tree", view: { content: {} } };
  for (const listener of listeners.active) listener({ panel: dock.activePanel });
  check("a non-editor active panel clears activeEditor", context.getValue("activeEditor") === undefined);
  check("a non-editor active panel clears editorLangId", context.getValue("editorLangId") === undefined);
  binding.dispose();
  markdownPanel.dispose();
}

{
  // The CodeMirror text-control adapter: the surface registers itself,
  // and the Edit menu's undo/select-all route to its history.
  const adapterSurface = new CodeMirrorSurface();
  window.document.body.appendChild(adapterSurface.element);
  adapterSurface.open({ path: "C:\\p\\t.txt", text: "one\ntwo\n" });
  const textControls = getService(TEXT_CONTROL_SERVICE);
  adapterSurface.focus();
  check("a focused editor surface activates its codemirror adapter", textControls.active?.kind === "codemirror");
  check(
    "editorTextFocus follows the focused editor",
    getService(CONTEXT_KEY_SERVICE).getValue("editorTextFocus") === true,
  );
  const adapterView = adapterSurface.editorView();
  adapterView.dispatch({ changes: { from: 0, insert: "x" } });
  textControls.undo();
  check("undo routes through the adapter to the editor's history", adapterSurface.text() === "one\ntwo\n");
  textControls.selectAll();
  check(
    "selectAll routes through the adapter",
    adapterView.state.selection.main.from === 0 &&
      adapterView.state.selection.main.to === adapterView.state.doc.length,
  );
  adapterSurface.dispose();
  check("disposing the surface unregisters its adapter", textControls.active === null);
}

{
  // The ":" go-to-line provider: parse, guidance row, and accept.
  const bare = parseLineColumn("12");
  check("parseLineColumn parses a bare line", bare?.line === 12 && bare?.column === undefined);
  const withColon = parseLineColumn("12:5");
  check("parseLineColumn parses line:column", withColon?.line === 12 && withColon?.column === 5);
  const withComma = parseLineColumn("12,5");
  check("parseLineColumn parses line,column", withComma?.line === 12 && withComma?.column === 5);
  check("parseLineColumn rejects non-numeric input", parseLineColumn("abc") === null);
  check("parseLineColumn rejects empty input", parseLineColumn("") === null);
  check("parseLineColumn rejects a zero line", parseLineColumn("0") === null);

  const provider = createGotoLineProvider();
  const guidance = provider.getItems("");
  check("an empty filter shows a guidance row", guidance.length === 1 && guidance[0].label.length > 0);
  const rows = provider.getItems("12");
  check("a line filter offers a go-to row", rows.length === 1 && rows[0].label.includes("12"));

  const gotoSurface = new CodeMirrorSurface();
  window.document.body.appendChild(gotoSurface.element);
  gotoSurface.open({
    path: "C:\\p\\g.txt",
    text: Array.from({ length: 20 }, (_, index) => `line ${index + 1}`).join("\n"),
  });
  const gotoPanel = new EditorPanel({ createSurface: () => gotoSurface });
  dock.activePanel = { id: "editor:C:\\p\\g.txt", view: { content: gotoPanel } };
  rows[0].accept();
  await flush();
  const gotoView = gotoSurface.editorView();
  check(
    "accepting a go-to row moves the cursor to the line",
    gotoView.state.selection.main.head === gotoView.state.doc.line(12).from,
  );
  editorCommands.goToLine(5, 2);
  check(
    "goToLine honors the column",
    gotoView.state.selection.main.head === gotoView.state.doc.line(5).from + 1,
  );
  editorCommands.goToLine(999);
  check(
    "goToLine clamps past the last line",
    gotoView.state.selection.main.head === gotoView.state.doc.line(20).from,
  );
  gotoPanel.dispose();
}

if (failures.length > 0) {
  console.error(`editor-commands: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-commands: all assertions passed");
process.exit(0);
