// The Add Folder to Workspace flow, shared by the Workshop tree panel's
// header button and empty-space context menu and by the File menu's
// Open Folder and Add Folder to Workspace rows; the workshop is
// multi-root, so both rows run this one flow. In the
// desktop app the native folder picker answers with the chosen path and
// a cancel answers nothing; in a plain browser, where no picker and no
// OS paths exist, a dialog asks for the path as text. A granted folder
// announces the workspace change so the tree refreshes, and the outcome
// paints the status bar, exactly as a native drop does.

import { open } from "@tauri-apps/plugin-dialog";

import type { IDisposable } from "../../base/lifecycle";
import { getServiceOrNull } from "../../services/service-registry";
import { showPanelDialog } from "../editor/editor-dialog";
import { STATUS_BAR } from "../../services/status-bar";
import { grantPath, WORKSPACE_CHANGED_EVENT } from "./workspace-drops";

/** The status-bar surface the flow paints action outcomes onto. */
export interface AddFolderStatusSink {
  showLocal(label: string, severity: "info" | "error"): void;
}

/**
 * Starts the flow over `host`, the dialog's overlay parent in browser
 * mode. Returns the dialog's disposable in browser mode - the caller
 * owns dismissal when its own lifetime ends - and null in the desktop
 * app, where the native picker needs no host. Outcomes paint `statusBar`
 * when given, else the status-bar service when one is registered.
 */
export function addFolderToWorkspace(
  host: HTMLElement,
  statusBar?: AddFolderStatusSink | null,
): IDisposable | null {
  if (window.__TAURI_INTERNALS__ !== undefined) {
    void pickFolder(statusBar);
    return null;
  }
  return showPanelDialog({
    host,
    classPrefix: "ws-workspace-add",
    titleId: "workspace-add-title",
    title: "Add Folder to Workspace",
    message: "Enter the full path of a folder to browse in the Workshop.",
    field: { id: "workspace-add-path", label: "Folder path" },
    buttons: [
      {
        label: "Add",
        requiresValue: true,
        run: (value) => {
          void grantFolder(value, statusBar);
        },
      },
      { label: "Cancel", run: () => undefined },
    ],
  });
}

/** The desktop pick: the native dialog; a cancel resolves null. */
async function pickFolder(statusBar: AddFolderStatusSink | null | undefined): Promise<void> {
  const picked = await open({ directory: true, title: "Add Folder to Workspace" });
  if (picked === null) {
    return;
  }
  await grantFolder(picked, statusBar);
}

/** Grants one folder and announces the outcome, like the drop flow. */
async function grantFolder(
  path: string,
  sink: AddFolderStatusSink | null | undefined,
): Promise<void> {
  const statusBar = sink ?? getServiceOrNull(STATUS_BAR);
  const result = await grantPath(path);
  if (!result.ok) {
    statusBar?.showLocal(`Could not add ${path}: ${result.error.message}`, "error");
    return;
  }
  statusBar?.showLocal(`Added ${path} to the Workshop`, "info");
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
}
