// Native file drops in the desktop shell. The page cannot read real OS
// paths from an HTML5 drop, so on drop it posts the DOM File objects over
// the WebView2 web-message channel (postMessageWithAdditionalObjects);
// the shell reads each file's real path and answers with a
// `promptforge:file-drop` event, which this module validates and grants
// through the workspace HTTP API. Desktop mode never reads file bytes
// merely because a file was dragged onto the window. In a plain browser
// neither the bridge nor the event exists and normal HTML drag/drop of
// file contents keeps working untouched.
//
// The shell never touches the OS drop itself (WebView2's own drop target
// is what keeps HTML5 drag-and-drop alive for Dockview), so the page must
// suppress the browser's default file-drop action - navigating away to
// the dropped file - itself. Only drags carrying files are suppressed;
// in-page drags (Dockview tabs) are untouched.

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";
import type { StatusBar } from "./status-bar";

/** The native event the shell dispatches when files land on the window. */
const FILE_DROP_EVENT = "promptforge:file-drop";

/** Fired on window after grants change, so open panels can refresh. */
export const WORKSPACE_CHANGED_EVENT = "promptforge:workspace-changed";

/** The web message the shell's file-drop bridge listens for. */
const DROP_MESSAGE = "workspace-drop";

/** The WebView2 script bridge, present only inside the desktop shell. */
interface WebView2Bridge {
  readonly postMessageWithAdditionalObjects?: (message: string, objects: readonly File[]) => void;
}

/**
 * Posts a drop's File objects to the shell, which reads their real OS
 * paths (something the page itself is never allowed to see) and answers
 * with the `promptforge:file-drop` event. Outside the desktop shell the
 * bridge does not exist and the drop ends here.
 */
function postDroppedFiles(event: DragEvent): void {
  const files = event.dataTransfer?.files;
  if (files === undefined || files.length === 0) {
    return;
  }
  const bridge = (window as { chrome?: { webview?: WebView2Bridge } }).chrome?.webview;
  bridge?.postMessageWithAdditionalObjects?.(DROP_MESSAGE, Array.from(files));
}

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

/**
 * Grants one path through the workspace API. Shared with the Workshop
 * tree's Add Folder flow, which grants picked and typed paths the same
 * way a drop does.
 */
export async function grantPath(path: string): Promise<void> {
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
 * bar and do not stop the remaining grants. Successful grants confirm on
 * the status bar and announce the change so the Workshop tree refreshes -
 * without this the drop works but nothing visible happens.
 */
async function grantDroppedPaths(
  paths: readonly string[],
  statusBar: StatusBar,
): Promise<void> {
  let granted = 0;
  for (const path of paths) {
    try {
      await grantPath(path);
      granted += 1;
      statusBar.showLocal(`Added ${path} to the Workshop`, "info");
    } catch (error) {
      statusBar.showLocal(`Could not open ${path}: ${(error as Error).message}`, "error");
    }
  }
  if (granted > 0) {
    window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
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
 * Returns the disposable owning every window listener wired here.
 */
export function setupWorkspaceDrops(statusBar: StatusBar): IDisposable {
  const store = new DisposableStore();
  const onDragOver = (event: DragEvent): void => {
    if (isFileDrag(event)) event.preventDefault();
  };
  window.addEventListener("dragover", onDragOver);
  store.add(toDisposable(() => window.removeEventListener("dragover", onDragOver)));
  const onDrop = (event: DragEvent): void => {
    if (isFileDrag(event)) {
      event.preventDefault();
      postDroppedFiles(event);
    }
  };
  window.addEventListener("drop", onDrop);
  store.add(toDisposable(() => window.removeEventListener("drop", onDrop)));
  if (window.__PROMPTFORGE_DESKTOP__ !== true) {
    return store;
  }
  const onFileDrop = (event: Event): void => {
    const paths = readDroppedPaths(event);
    if (paths === null || paths.length === 0) {
      return;
    }
    void grantDroppedPaths(paths, statusBar).catch((error: unknown) => {
      statusBar.showLocal(`Could not open the dropped files: ${(error as Error).message}`, "error");
    });
  };
  window.addEventListener(FILE_DROP_EVENT, onFileDrop);
  store.add(toDisposable(() => window.removeEventListener(FILE_DROP_EVENT, onFileDrop)));
  return store;
}
