// The closed-editor stack behind Reopen Closed Editor. The stack belongs
// to the workspace: it seeds from the workspace bucket's
// "closed_editors" value at construction and writes the same shape,
// `{ "paths": [...] }` most recent first, back through its writer on
// every push and pop, so the files a user closed come back on relaunch.
//
// Only file closes persist. An untitled buffer's close keeps its text on
// the in-memory stack for the session (reopening restores the draft) but
// never reaches the writer: unsaved text is not persisted by design.
//
// The initial value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as an empty stack, never
// as a cast. The writer is fire-and-forget: a write that throws is
// swallowed and the in-memory stack stays authoritative.
//
// The stack self-registers with a default factory (empty, no-op writer),
// so any bundle that touches it gets a working singleton; the composition
// root re-registers it bound to the live adapter before the editor chunk
// installs its dock tracking. This module stays CodeMirror-free, apart
// from editor-lifecycle.ts, so main.ts can bind the token without pulling
// the editor chunk into the initial bundle.

import { CLOSED_EDITORS, type ClosedEditor, type ClosedEditorsSnapshot } from "../../services/closed-editors";
import { registerService } from "../../services/service-registry";

/** The most closed editors the stack retains; older entries drop. */
const MAX_CLOSED_EDITORS = 50;

/** The writer the stack hands `{ paths: [...] }` to after a change. */
export type ClosedEditorsWriter = (value: unknown) => void;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Narrows a most-recent-first path list into the in-memory stack order
 * (oldest first, so the reopen is a pop): non-string and empty entries
 * drop out, and only the most recent MAX_CLOSED_EDITORS survive.
 */
function stackFromPaths(value: unknown): ClosedEditor[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const recentFirst: ClosedEditor[] = [];
  for (const item of value as readonly unknown[]) {
    if (typeof item === "string" && item !== "") {
      recentFirst.push({ kind: "file", path: item });
    }
    if (recentFirst.length === MAX_CLOSED_EDITORS) {
      break;
    }
  }
  return recentFirst.reverse();
}

/**
 * Narrows the persisted payload to the stack: it must be an object
 * with a `paths` array. Anything else reads as an empty stack.
 */
function readInitial(initial: unknown): ClosedEditor[] {
  return isRecord(initial) ? stackFromPaths(initial.paths) : [];
}

/**
 * The closed-editor stack. Push records a close, pop yields the most
 * recent one for reopening; both write the file-path snapshot through
 * the injected writer.
 */
export class ClosedEditors {
  // Oldest first: the most recent close is last, so the reopen is a pop.
  private stack: ClosedEditor[];

  /**
   * `initial` is the value the workspace bucket held at boot (any shape;
   * see readInitial); `write` receives `{ paths: [...] }` after each push
   * or pop.
   */
  constructor(
    initial: unknown = null,
    private readonly write: ClosedEditorsWriter = () => {},
  ) {
    this.stack = readInitial(initial);
  }

  /** Records a closing editor; the oldest entry drops past the cap. */
  push(entry: ClosedEditor): void {
    this.stack.push(entry);
    if (this.stack.length > MAX_CLOSED_EDITORS) {
      this.stack.shift();
    }
    this.persist();
  }

  /** Removes and returns the most recently closed editor; undefined when none. */
  pop(): ClosedEditor | undefined {
    const entry = this.stack.pop();
    if (entry !== undefined) {
      this.persist();
    }
    return entry;
  }

  /** The persisted view: file paths, most recent first; Save As writes it. */
  snapshot(): ClosedEditorsSnapshot {
    const paths: string[] = [];
    for (let i = this.stack.length - 1; i >= 0; i -= 1) {
      const entry = this.stack[i];
      if (entry !== undefined && entry.kind === "file") {
        paths.push(entry.path);
      }
    }
    return { paths };
  }

  /**
   * Replaces the stack wholesale with a workspace's stored paths (Open
   * Workspace from File), most recent first. Writes nothing: the paths
   * came from the file, so echoing them back would be a second writer.
   */
  replaceClosedEditors(paths: readonly string[]): void {
    this.stack = stackFromPaths(paths);
  }

  private persist(): void {
    try {
      this.write(this.snapshot());
    } catch {
      // The adapter reports its own failures; a throwing writer leaves
      // the in-memory stack authoritative.
    }
  }
}

// Self-registration with an empty stack and a no-op writer: a consumer
// that resolves the token before the composition root re-registers it
// bound to the live adapter gets a working, unpersisted stack rather than
// a wrong instance cached for the page lifetime.
registerService(CLOSED_EDITORS, () => new ClosedEditors());
