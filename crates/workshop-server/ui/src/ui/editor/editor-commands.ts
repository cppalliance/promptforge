// The editor's command layer, in two halves.
//
// Workshop-level commands - save the active editor, close it (prompting
// on unsaved changes), and cycle the open editors - resolve the dock
// through the service registry (the DOCK token, registered by
// initZones) instead of capturing it.
//
// The CodeMirror-backed catalog rows (plan step 13): runInActiveEditor
// and withActiveEditor resolve the active editor through the dock, the
// built-in CodeMirror commands are re-exported so the contribution file
// has exactly one lazy import site, and the custom StateCommands
// (duplicate selection, cursor add rows, previous occurrence) plus the
// smart-select selection stack live here beside save/close/cycle.
// Keeping every CodeMirror import in this module is what lets
// editor.contribution.ts stay out of the initial bundle.

import type { IDockviewPanel } from "dockview";
import { EditorSelection, type StateCommand } from "@codemirror/state";
import type { Command, EditorView } from "@codemirror/view";
import {
  copyLineDown,
  copyLineUp,
  cursorMatchingBracket,
  moveLineDown,
  moveLineUp,
  selectParentSyntax,
  toggleBlockComment,
  toggleLineComment,
} from "@codemirror/commands";
import { nextDiagnostic, previousDiagnostic } from "@codemirror/lint";
import { openSearchPanel, SearchCursor, selectNextOccurrence, selectSelectionMatches } from "@codemirror/search";

import { DOCK, resolvePanelContent } from "../../services/panel-registry";
import { getService } from "../../services/service-registry";
import { EditorPanel } from "./editor-panel";

// The built-in CodeMirror commands behind the catalog rows, re-exported
// so editor.contribution.ts lazy-imports this module and nothing else.
export {
  copyLineDown,
  copyLineUp,
  cursorMatchingBracket,
  moveLineDown,
  moveLineUp,
  nextDiagnostic,
  openSearchPanel,
  previousDiagnostic,
  selectNextOccurrence,
  selectSelectionMatches,
  toggleBlockComment,
  toggleLineComment,
};

/** The panel's content as an EditorPanel, or null for other panel kinds. */
function asEditor(panel: IDockviewPanel | undefined): EditorPanel | null {
  if (panel === undefined) {
    return null;
  }
  // view.content may be the lazy wrapper while the chunk loads; unwrap
  // to the real panel before the instanceof check.
  const content = resolvePanelContent(panel.view.content);
  return content instanceof EditorPanel ? content : null;
}

/** Every open editor panel, in dock order. */
function editorPanels(): IDockviewPanel[] {
  return getService(DOCK).panels.filter((panel) => asEditor(panel) !== null);
}

/** Ctrl+S: save the active editor. A no-op when no editor is active. */
export function saveActiveEditor(): void {
  const editor = asEditor(getService(DOCK).activePanel);
  if (editor !== null) {
    // save() handles its own failures (error bar, conflict dialog).
    void editor.save();
  }
}

/** Ctrl+W: close the active editor, prompting on unsaved changes. */
export function closeActiveEditor(): void {
  asEditor(getService(DOCK).activePanel)?.requestClose();
}

/** Ctrl+Tab / Ctrl+Shift+Tab: cycle the open editors, wrapping around. */
export function cycleEditor(direction: 1 | -1): void {
  const dock = getService(DOCK);
  const editors = editorPanels();
  if (editors.length === 0) {
    return;
  }
  const current = editors.findIndex((panel) => panel === dock.activePanel);
  const index =
    current === -1
      ? direction === 1
        ? 0
        : editors.length - 1
      : (current + direction + editors.length) % editors.length;
  const panel = editors[index];
  if (panel === undefined) {
    return;
  }
  panel.api.setActive();
  asEditor(panel)?.focus();
}

/**
 * Runs a CodeMirror command against the active editor's view. Returns
 * false when no editor is active or the command declines.
 */
export function runInActiveEditor(command: Command | StateCommand): boolean {
  const view = asEditor(getService(DOCK).activePanel)?.editorView() ?? null;
  if (view === null) {
    return false;
  }
  return command(view);
}

/** Runs `fn` with the active editor panel; a no-op when none is active. */
export function withActiveEditor(fn: (panel: EditorPanel) => void): void {
  const editor = asEditor(getService(DOCK).activePanel);
  if (editor !== null) {
    fn(editor);
  }
}

/** Replace: opens the search panel and moves focus to the replace field. */
export const startFindReplace: Command = (view) => {
  if (!openSearchPanel(view)) {
    return false;
  }
  const replace = view.dom.querySelector("input[name=replace]");
  if (replace instanceof HTMLInputElement) {
    replace.focus();
    replace.select();
  }
  return true;
};

// The expand/shrink selection stack, per view: expand pushes the
// pre-expansion selection, shrink pops and restores it. A WeakMap so the
// stack dies with the view.
const selectionStacks = new WeakMap<EditorView, EditorSelection[]>();

/** Expand Selection: selects the parent syntax node, pushing the prior selection. */
export const smartSelectExpand: Command = (view) => {
  const before = view.state.selection;
  if (!selectParentSyntax({ state: view.state, dispatch: (tr) => view.dispatch(tr) })) {
    return false;
  }
  const stack = selectionStacks.get(view) ?? [];
  stack.push(before);
  selectionStacks.set(view, stack);
  return true;
};

/** Shrink Selection: restores the selection the last expand pushed; an empty stack is a no-op. */
export const smartSelectShrink: Command = (view) => {
  const previous = selectionStacks.get(view)?.pop();
  if (previous === undefined) {
    return false;
  }
  // The document may have changed since the push; clamp into range.
  const length = view.state.doc.length;
  view.dispatch({
    selection: EditorSelection.create(
      previous.ranges.map((range) =>
        EditorSelection.range(Math.min(range.anchor, length), Math.min(range.head, length)),
      ),
    ),
    userEvent: "select",
  });
  return true;
};

/**
 * Duplicate Selection: an all-empty selection copies the line down;
 * otherwise the selected text is inserted after each range and the
 * copies become the selection.
 */
export const duplicateSelection: StateCommand = ({ state, dispatch }) => {
  if (state.selection.ranges.every((range) => range.empty)) {
    return copyLineDown({ state, dispatch });
  }
  const changes = state.selection.ranges.map((range) => ({
    from: range.to,
    insert: state.sliceDoc(range.from, range.to),
  }));
  const changeSet = state.changes(changes);
  const ranges = state.selection.ranges.map((range) => {
    const from = changeSet.mapPos(range.to, -1);
    return EditorSelection.range(from, from + (range.to - range.from));
  });
  dispatch(
    state.update({
      changes: changeSet,
      selection: EditorSelection.create(ranges),
      scrollIntoView: true,
      userEvent: "input",
    }),
  );
  return true;
};

/** Builds Add Cursor Above/Below: one new cursor per head, one line over, same column. */
function addCursorLine(direction: -1 | 1): StateCommand {
  return ({ state, dispatch }) => {
    const ranges = [...state.selection.ranges];
    let added = false;
    for (const range of state.selection.ranges) {
      const line = state.doc.lineAt(range.head);
      const target = line.number + direction;
      if (target < 1 || target > state.doc.lines) {
        continue;
      }
      const targetLine = state.doc.line(target);
      const head = Math.min(targetLine.from + (range.head - line.from), targetLine.to);
      ranges.push(EditorSelection.cursor(head));
      added = true;
    }
    if (!added) {
      return false;
    }
    dispatch(state.update({ selection: EditorSelection.create(ranges), userEvent: "select" }));
    return true;
  };
}

/** Add Cursor Above: a cursor one line up at the same column for each head. */
export const insertCursorAbove: StateCommand = addCursorLine(-1);

/** Add Cursor Below: a cursor one line down at the same column for each head. */
export const insertCursorBelow: StateCommand = addCursorLine(1);

/** Add Cursors to Line Ends: one cursor at the end of every line the selection touches. */
export const insertCursorAtLineEnds: StateCommand = ({ state, dispatch }) => {
  const cursors: number[] = [];
  for (const range of state.selection.ranges) {
    const firstLine = state.doc.lineAt(range.from).number;
    let lastLine = state.doc.lineAt(range.to).number;
    // A selection ending exactly at a line start does not touch that line.
    if (!range.empty && lastLine > firstLine && range.to === state.doc.line(lastLine).from) {
      lastLine -= 1;
    }
    for (let n = firstLine; n <= lastLine; n++) {
      const end = state.doc.line(n).to;
      if (!cursors.includes(end)) {
        cursors.push(end);
      }
    }
  }
  dispatch(
    state.update({
      selection: EditorSelection.create(cursors.map((pos) => EditorSelection.cursor(pos))),
      userEvent: "select",
    }),
  );
  return true;
};

/**
 * Add Previous Occurrence: the mirror of selectNextOccurrence - adds the
 * closest match before the first range, wrapping to the document's last
 * match. All ranges must select the same text, as selectNextOccurrence
 * requires; a single already-selected match is a no-op.
 */
export const selectPreviousOccurrence: StateCommand = ({ state, dispatch }) => {
  const { ranges } = state.selection;
  if (ranges.length === 0 || ranges.some((range) => range.empty)) {
    return false;
  }
  const first = ranges[0];
  const searched = state.sliceDoc(first.from, first.to);
  if (ranges.some((range) => state.sliceDoc(range.from, range.to) !== searched)) {
    return false;
  }
  const cursor = new SearchCursor(state.doc, searched);
  let before: { from: number; to: number } | null = null;
  let last: { from: number; to: number } | null = null;
  for (let match = cursor.next(); !match.done; match = cursor.next()) {
    const found = { from: match.value.from, to: match.value.to };
    if (found.from < first.from) {
      before = found;
    }
    last = found;
  }
  const target = before ?? last;
  if (target === null || ranges.some((range) => range.from === target.from && range.to === target.to)) {
    return false;
  }
  dispatch(
    state.update({
      selection: EditorSelection.create([...ranges, EditorSelection.range(target.from, target.to)]),
      userEvent: "select.search",
    }),
  );
  return true;
};
