// The workspace-files contribution: the eager module registering the
// File menu's workspace-document rows (plan step 10) at module scope,
// before any service exists. Open Workspace from File... takes over the
// stub table's row under the same command id and label, so no menu,
// test, or keybinding wiring changes; the row is desktop-only
// (precondition !isWeb) because its picker is the native Tauri dialog.
//
// The run body lazy-imports the dialog plugin and the Tauri event API,
// so this module pulls neither into the initial bundle. A successful
// open replaces the grants wholesale on the server; the page then fires
// the existing promptforge:workspace-changed invalidation so the tree
// refreshes, emits the promptforge:workspace-opened Tauri event so the
// shell can fetch and apply the file's window geometry (native geometry
// is the shell's to apply, never the page's), and records the file in
// the recent-files store. Failures paint the status bar, exactly as the
// other file actions do; success is silent, the refreshed tree being
// its own confirmation; a cancelled picker is a no-op.

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import { errorText, type Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { RECENT_FILES_STORE } from "../../services/recent-files-store";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { openWorkspaceFile } from "../../services/workspace-file-client";
import { STATUS_BAR } from "../status/status-bar";
import { WORKSPACE_CHANGED_EVENT } from "../workspace/workspace-drops";

/** The Tauri event the shell listens for to re-apply window geometry. */
export const WORKSPACE_OPENED_EVENT = "promptforge:workspace-opened";

/**
 * The feature's action registrations, owned as one disposable so the
 * barrel's register() can hand them to the composition root's
 * ownership tree; the other contributions leave theirs to the page.
 */
export const registrations = new DisposableStore();

/** The picker filter: workspace files only. */
const WORKSPACE_FILTERS = [{ name: "PromptForge Workspace", extensions: ["pfwork"] }];

/** Paints an action failure onto the status bar, when one is up. */
function reportError(label: string): void {
  getServiceOrNull(STATUS_BAR)?.showLocal(label, "error");
}

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`workspace-files action '${action.id}': ${result.error.message}`);
    return;
  }
  registrations.add(result.value);
}

/**
 * Tells the shell a workspace file is now current. The event only
 * matters for geometry, which the open already committed, so a failed
 * emit is logged and never undoes the open.
 */
async function announceOpened(path: string): Promise<void> {
  try {
    const { emit } = await import("@tauri-apps/api/event");
    await emit(WORKSPACE_OPENED_EVENT, { path });
  } catch (error) {
    console.warn(`${WORKSPACE_OPENED_EVENT}: ${errorText(error)}`);
  }
}

/**
 * Open Workspace from File...: the native picker filtered to .pfwork,
 * the open on the server, then the invalidation, the shell event, and
 * the recent entry. A cancelled picker answers a non-string and is a
 * no-op; a refusal reports and changes nothing on the page.
 */
async function openWorkspaceFromFile(): Promise<void> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ title: "Open Workspace from File", filters: WORKSPACE_FILTERS });
  if (typeof picked !== "string") {
    return;
  }
  try {
    await openWorkspaceFile(picked);
  } catch (error) {
    reportError(`Could not open ${picked}: ${errorText(error)}`);
    return;
  }
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  await announceOpened(picked);
  getService(RECENT_FILES_STORE).add(picked);
}

addAction({
  id: "workbench.action.openWorkspace",
  title: "Open Workspace from File...",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open", order: 3 }],
  run: openWorkspaceFromFile,
});
