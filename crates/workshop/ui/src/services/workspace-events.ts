// The workspace window events shared across parts: the grant sources
// (drops, the Add Folder flow, the file and workspace-document actions)
// fire them, and the tree and the window title listen.

/** Fired on window after grants change, so open panels can refresh. */
export const WORKSPACE_CHANGED_EVENT = "promptforge:workspace-changed";
