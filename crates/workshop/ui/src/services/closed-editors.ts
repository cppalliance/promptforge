// The closed-editor service contract behind Reopen Closed Editor. The
// implementation (parts/editor/closed-editors.ts) is workspace-persisted
// and self-registers with a default factory, but it stays in parts because
// the composition root binds it to the live adapter beside the other
// workspace-scoped stores; only the token and the stack's public shapes
// live here, in the DOM-free services layer.

import { createServiceToken, type ServiceToken } from "./service-registry";

/** One closed editor: a file to reopen by path, or an untitled buffer's text. */
export type ClosedEditor =
  | { readonly kind: "file"; readonly path: string }
  | { readonly kind: "untitled"; readonly text: string };

/** The persisted shape: file paths only, most recent first. */
export interface ClosedEditorsSnapshot {
  readonly paths: string[];
}

/** The closed-editor stack consumers resolve from the registry. */
export interface ClosedEditors {
  /** Records a closing editor; the oldest entry drops past the cap. */
  push(entry: ClosedEditor): void;
  /** Removes and returns the most recently closed editor; undefined when none. */
  pop(): ClosedEditor | undefined;
  /** The persisted view: file paths, most recent first; Save As writes it. */
  snapshot(): ClosedEditorsSnapshot;
  /**
   * Replaces the stack wholesale with a workspace's stored paths (Open
   * Workspace from File), most recent first, without a write.
   */
  replaceClosedEditors(paths: readonly string[]): void;
}

/** The registry token for the closed-editor stack singleton. */
export const CLOSED_EDITORS = createServiceToken<ClosedEditors>("workshop.closedEditors");
