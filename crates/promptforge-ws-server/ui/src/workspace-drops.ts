// Native file drops from the desktop shell. The shell's delegating drop
// target turns an Explorer drop into a `promptforge:file-drop` event
// carrying real OS paths; this module validates the payload and grants
// each path through the workspace HTTP API. Desktop mode never reads file
// bytes merely because a file was dragged onto the window. In a plain
// browser the event never fires and normal HTML drag/drop of file
// contents keeps working untouched.
//
// Because the shell forwards the OS drop to the webview (that forwarding
// is what keeps HTML5 drag-and-drop alive for Dockview), the page must
// suppress the browser's default file-drop action - navigating away to
// the dropped file - itself. Only drags carrying files are suppressed;
// in-page drags (Dockview tabs) are untouched.

import type { StatusBar } from "./status-bar";

/** The native event the shell dispatches when files land on the window. */
const FILE_DROP_EVENT = "promptforge:file-drop";

/**
 * Reads the dropped paths out of the native event. The detail arrives as
 * `unknown` and is validated field by field; anything that is not an
 * object carrying a `paths` array of plain strings is rejected.
 */
function readDroppedPaths(event: Event): readonly string[] | null {
  if (!(event instanceof CustomEvent)) {
    return null;
  }
  const detail: unknown = event.detail;
  if (typeof detail !== "object" || detail === null || !("paths" in detail)) {
    return null;
  }
  const { paths } = detail;
  if (!Array.isArray(paths) || !paths.every((path) => typeof path === "string")) {
    return null;
  }
  return paths;
}

/** Extracts the server's error message from a failed grant response. */
function grantErrorMessage(body: unknown, status: number): string {
  if (typeof body === "object" && body !== null && "error" in body) {
    const { error } = body;
    if (
      typeof error === "object" &&
      error !== null &&
      "message" in error &&
      typeof error.message === "string"
    ) {
      return error.message;
    }
  }
  return `POST /workspace/grant answered ${status}`;
}

/** Grants one dropped path through the workspace API. */
async function grantPath(path: string): Promise<void> {
  const response = await fetch("/workspace/grant", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path }),
  });
  const body: unknown = await response.json();
  if (!response.ok) {
    throw new Error(grantErrorMessage(body, response.status));
  }
}

/**
 * Grants every dropped path in order. Per-path failures paint the status
 * bar and do not stop the remaining grants.
 */
async function grantDroppedPaths(
  paths: readonly string[],
  statusBar: StatusBar,
): Promise<void> {
  for (const path of paths) {
    try {
      await grantPath(path);
    } catch (error) {
      statusBar.showLocal(`Could not open ${path}: ${(error as Error).message}`, "error");
    }
  }
}

/** True when a drag carries OS files rather than an in-page payload. */
function isFileDrag(event: DragEvent): boolean {
  const types = event.dataTransfer?.types;
  return types !== undefined && Array.from(types).includes("Files");
}

/**
 * Listens for the shell's native drop event and grants each dropped path
 * through the workspace API. Active only in the desktop shell; in a plain
 * browser there is no native drop source and the grant listener is never
 * installed. The file-drag default suppression is installed everywhere:
 * dropping a file must never navigate the page away, desktop or browser.
 */
export function setupWorkspaceDrops(statusBar: StatusBar): void {
  window.addEventListener("dragover", (event) => {
    if (isFileDrag(event)) event.preventDefault();
  });
  window.addEventListener("drop", (event) => {
    if (isFileDrag(event)) event.preventDefault();
  });
  if (window.__PROMPTFORGE_DESKTOP__ !== true) {
    return;
  }
  window.addEventListener(FILE_DROP_EVENT, (event) => {
    const paths = readDroppedPaths(event);
    if (paths === null || paths.length === 0) {
      return;
    }
    void grantDroppedPaths(paths, statusBar).catch((error: unknown) => {
      statusBar.showLocal(`Could not open the dropped files: ${(error as Error).message}`, "error");
    });
  });
}
