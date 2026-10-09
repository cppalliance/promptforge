// The workspace window events shared across parts: the grant sources
// (drops, the Add Folder flow, the file and workspace-document actions)
// fire them, and the tree-state service, the tree, the window title, and
// the editor watermark listen.

import { isRecord } from "./json-request";

/** Fired on window after grants change, so open panels can refresh. */
export const WORKSPACE_CHANGED_EVENT = "promptforge:workspace-changed";

/**
 * The optional detail of a workspace-changed event. A plain event means
 * the grants changed since any roots load in flight started, and the
 * tree-state service drops its roots listing so the next read fetches
 * again. `rootsCurrent` says the listing the service holds or is loading
 * already describes the new grants: Open Workspace from File invalidates
 * the roots before it re-creates the tree panel, whose init starts the one
 * load, and Save As and Duplicate keep the grants. The service keeps that
 * load instead of dropping it, so a switch fetches the roots once.
 */
export interface WorkspaceChangedDetail {
  readonly rootsCurrent: boolean;
}

/** Whether a workspace-changed event says the roots listing is already current. */
export function rootsCurrentIn(event: Event): boolean {
  const detail: unknown = event instanceof CustomEvent ? event.detail : null;
  return isRecord(detail) && detail.rootsCurrent === true;
}
