// The File menu's picker and file-action run bodies (plan step 16),
// lazy-loaded by files.contribution.ts so the contribution module stays
// out of the dockview and CodeMirror import graph. Open File and Save As
// are desktop-only rows (precondition !isWeb): their pickers are the
// native Tauri dialogs. Open Folder and Add Folder to Workspace share
// the tree's Add Folder flow, which falls back to a typed-path dialog in
// a plain browser. A cancelled picker is always a no-op.

import { open, save } from "@tauri-apps/plugin-dialog";

import { DOCK } from "../../services/panel-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { asEditor } from "../editor/editor-commands";
import { openInZone } from "../layout/zones";
import { STATUS_BAR } from "../status/status-bar";
import { addFolderToWorkspace } from "./add-folder";
import { grantPath, WORKSPACE_CHANGED_EVENT } from "./workspace-drops";

/** Paints an action outcome onto the status bar, when one is up. */
function report(label: string, severity: "info" | "error"): void {
  getServiceOrNull(STATUS_BAR)?.showLocal(label, severity);
}

/** The directory portion of a path, or null when it has no separator. */
function parentDir(path: string): string | null {
  const match = /^(.*)[\\/][^\\/]+$/.exec(path);
  return match?.[1] ?? null;
}

/**
 * Open File...: the native file picker, a grant so the confined
 * workspace API can read the pick, then an editor on the path. A cancel
 * answers a non-string and is a no-op.
 */
export async function openFile(): Promise<void> {
  const picked = await open({ multiple: false, title: "Open File" });
  if (typeof picked !== "string") {
    return;
  }
  const granted = await grantPath(picked);
  if (!granted.ok) {
    report(`Could not open ${picked}: ${granted.error.message}`, "error");
    return;
  }
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  openInZone("editor", { path: picked });
}

/** Open Folder...: the shared Add Folder flow. */
export function openFolder(): void {
  addFolderToWorkspace(document.body);
}

/** Add Folder to Workspace...: the workshop is multi-root, so this is Open Folder's flow. */
export function addRootFolder(): void {
  openFolder();
}

/**
 * Save As...: the native save dialog seeded with the current path, a
 * grant of the target's parent so the confined write reaches it, then
 * the active editor retargets onto the new path. A cancel is a no-op.
 */
export async function saveActiveEditorAs(): Promise<void> {
  const editor = asEditor(getService(DOCK).activePanel);
  if (editor === null) {
    return;
  }
  const current = editor.filePath();
  const picked = await save({
    title: "Save As",
    ...(current === null ? {} : { defaultPath: current }),
  });
  if (picked === null) {
    return;
  }
  const parent = parentDir(picked);
  if (parent !== null) {
    const granted = await grantPath(parent);
    if (!granted.ok) {
      report(`Could not save to ${picked}: ${granted.error.message}`, "error");
      return;
    }
  }
  await editor.saveAs(picked);
}

/**
 * Save All: every dirty editor panel, saved sequentially with for...of -
 * never Promise.all or an async forEach: the save path is race-pinned
 * by editor-save-race.mjs.
 */
export async function saveAllEditors(): Promise<void> {
  for (const panel of getService(DOCK).panels) {
    const editor = asEditor(panel);
    if (editor !== null && editor.isDirty()) {
      await editor.save();
    }
  }
}

/** Revert File: reload the active editor from disk, prompting on unsaved changes. */
export function revertActiveEditor(): void {
  asEditor(getService(DOCK).activePanel)?.requestRevert();
}
