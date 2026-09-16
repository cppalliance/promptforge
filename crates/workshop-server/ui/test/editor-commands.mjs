// Unit test for the editor commands catalog (plan step 13,
// src/ui/editor/editor-commands.ts and editor.contribution.ts). Every
// CodeMirror-backed catalog row is driven against a real EditorState -
// comment toggles, line copy/move, duplicate selection, cursor add rows,
// occurrence rows, bracket jump - or against a real EditorView in jsdom
// for the view commands (find, replace, smart select with its selection
// stack, diagnostic navigation). runInActiveEditor and withActiveEditor
// run against a stub dock, and the contribution's registry wiring (menu
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
      export { EditorState, EditorSelection } from "@codemirror/state";
      export { EditorView } from "@codemirror/view";
      export { javascript } from "@codemirror/lang-javascript";
      export { ensureSyntaxTree } from "@codemirror/language";
      export { setDiagnostics } from "@codemirror/lint";
      export { registerService } from "./src/services/service-registry.ts";
      export { DOCK } from "./src/services/panel-registry.ts";
      export { EditorPanel } from "./src/ui/editor/editor-panel.ts";
      export { CodeMirrorSurface } from "./src/ui/editor/editor-surface.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus, MenuId } from "./src/services/menu-registry.ts";
      export { KeybindingsRegistry } from "./src/services/keybinding-registry.ts";
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
  EditorState,
  EditorSelection,
  EditorView,
  javascript,
  ensureSyntaxTree,
  setDiagnostics,
  registerService,
  DOCK,
  EditorPanel,
  CodeMirrorSurface,
  Commands,
  Menus,
  MenuId,
  KeybindingsRegistry,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
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
  ];
  check("the contribution registered without errors", consoleErrors.length === 0);
  check(
    "every catalog row is a registered command",
    EXPECTED_IDS.every((id) => Commands.lookup(id) !== undefined),
  );
  const palette = Menus.getMenuItems(MenuId.CommandPalette).map((row) => row.command);
  // The twenty catalog rows plus step 14's four settings toggles.
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
    "the Go menu carries bracket jump and problem navigation",
    goMenu.length === 3 &&
      ["editor.action.jumpToBracket", "editor.action.marker.nextInFiles", "editor.action.marker.prevInFiles"]
        .every((id) => goMenu.includes(id)),
  );

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

if (failures.length > 0) {
  console.error(`editor-commands: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-commands: all assertions passed");
process.exit(0);
