// The ":" go-to-line quick-access provider. The provider itself is
// CodeMirror-free so the editor contribution can register it at module
// scope without pulling the editor chunk into the initial bundle; only
// the accept path lazy-imports the command layer. The filter syntax is
// VS Code's: a 1-based line number with an optional column after a
// colon or comma (":12", ":12:5", ":12,5").

import type { QuickAccessProvider, QuickInputItem } from "../quickinput/quick-input";

/** A parsed go-to target: a 1-based line and an optional 1-based column. */
export interface LineColumnTarget {
  readonly line: number;
  readonly column: number | undefined;
}

/** Parses a go-to-line filter, or null when it names no valid target. */
export function parseLineColumn(filter: string): LineColumnTarget | null {
  const match = /^\s*(\d+)(?:\s*[:,]\s*(\d+))?\s*$/.exec(filter);
  if (match === null || match[1] === undefined) {
    return null;
  }
  const line = Number(match[1]);
  const column = match[2] === undefined ? undefined : Number(match[2]);
  if (line < 1 || (column !== undefined && column < 1)) {
    return null;
  }
  return { line, column };
}

/**
 * The ":" provider: one go-to row for a valid target, one inert
 * guidance row otherwise. Accepting dispatches goToLine against the
 * active editor through the lazy command-layer import.
 */
export function createGotoLineProvider(): QuickAccessProvider {
  return {
    getItems(filter: string): readonly QuickInputItem[] {
      const target = parseLineColumn(filter);
      if (target === null) {
        return [{ label: "Type a line number to go to", accept: () => {} }];
      }
      const label =
        target.column === undefined
          ? `Go to line ${target.line}`
          : `Go to line ${target.line}, character ${target.column}`;
      return [
        {
          label,
          accept: () => {
            void import("./editor-commands").then((commands) => {
              commands.goToLine(target.line, target.column);
            });
          },
        },
      ];
    },
  };
}
