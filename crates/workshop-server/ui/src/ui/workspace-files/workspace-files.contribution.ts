// The workspace-files contribution: the eager module registering the
// File menu's workspace-document rows (plan steps 10 and 11) at module
// scope, before any service exists. Open Workspace from File..., Save
// Workspace As..., and Duplicate Workspace... take over the stub table's
// rows under the same command ids and labels (Duplicate gaining the
// ellipsis its save picker warrants), so no menu, test, or keybinding
// wiring changes; the rows are desktop-only (precondition !isWeb)
// because their pickers are the native Tauri dialogs.
//
// The run bodies lazy-import the dialog plugin and the Tauri event API,
// so this module pulls neither into the initial bundle. Every action is
// a switch: the server replaces the grants wholesale (open) or writes a
// new file and moves onto it (save as, duplicate); the page then fires
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
import {
  currentWorkspaceFile,
  duplicateWorkspaceFile,
  openWorkspaceFile,
  saveWorkspaceFileAs,
  type WorkspaceFileResponse,
} from "../../services/workspace-file-client";
import { STATUS_BAR } from "../status/status-bar";
import { WORKSPACE_CHANGED_EVENT } from "../workspace/workspace-drops";

/** The Tauri event the shell listens for to re-apply window geometry. */
export const WORKSPACE_OPENED_EVENT = "promptforge:workspace-opened";

/** The workspace file extension, as the pickers filter and the save paths end. */
const WORKSPACE_EXTENSION = ".pfwork";

/** The name the server reports while the workspace is ephemeral; the picker's fallback seed. */
const UNTITLED = "Untitled";

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
 * The page's side of a committed switch, shared by every action: the
 * tree invalidation, the shell event, and the recent entry. Runs only
 * after the server has answered success, so nothing here can undo it.
 */
async function announceSwitched(path: string): Promise<void> {
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  await announceOpened(path);
  getService(RECENT_FILES_STORE).add(path);
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
  await announceSwitched(picked);
}

/**
 * Appends .pfwork to a picked path that lacks it. Case-insensitive so a
 * Windows pick of `Name.PFWORK` is not doubled into `Name.PFWORK.pfwork`;
 * the extension is appended at most once.
 */
function withWorkspaceExtension(path: string): string {
  return path.toLowerCase().endsWith(WORKSPACE_EXTENSION) ? path : `${path}${WORKSPACE_EXTENSION}`;
}

/**
 * The current workspace's display name, seeding the save picker. A
 * failed lookup falls back to the ephemeral name rather than blocking
 * the picker: the write that follows reports its own failure if the
 * server is truly unreachable.
 */
async function currentWorkspaceName(): Promise<string> {
  try {
    return (await currentWorkspaceFile()).name;
  } catch (error) {
    console.warn(`workspace name lookup: ${errorText(error)}`);
    return UNTITLED;
  }
}

/**
 * One save-picker switch, shared by Save As and Duplicate: the native
 * save dialog seeded with the current name, the extension normalized,
 * the switch posted, then the page's announcement. A cancelled picker
 * answers null and is a no-op; a refusal reports and changes nothing.
 */
async function switchThroughSavePicker(
  title: string,
  verb: string,
  post: (path: string) => Promise<WorkspaceFileResponse>,
): Promise<void> {
  const name = await currentWorkspaceName();
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title,
    defaultPath: `${name}${WORKSPACE_EXTENSION}`,
    filters: WORKSPACE_FILTERS,
  });
  if (typeof picked !== "string") {
    return;
  }
  const target = withWorkspaceExtension(picked);
  try {
    await post(target);
  } catch (error) {
    reportError(`Could not ${verb} ${target}: ${errorText(error)}`);
    return;
  }
  await announceSwitched(target);
}

/** Save Workspace As...: a new file holding the current grants, then the switch onto it. */
function saveWorkspaceAs(): Promise<void> {
  return switchThroughSavePicker("Save Workspace As", "save workspace as", saveWorkspaceFileAs);
}

/** Duplicate Workspace...: a copy of the current file and its siblings, then the switch onto it. */
function duplicateWorkspace(): Promise<void> {
  return switchThroughSavePicker("Duplicate Workspace", "duplicate workspace to", duplicateWorkspaceFile);
}

addAction({
  id: "workbench.action.openWorkspace",
  title: "Open Workspace from File...",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "2_open", order: 3 }],
  run: openWorkspaceFromFile,
});

addAction({
  id: "workbench.action.saveWorkspaceAs",
  title: "Save Workspace As...",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "3_workspace", order: 2 }],
  run: saveWorkspaceAs,
});

addAction({
  id: "workbench.action.duplicateWorkspace",
  title: "Duplicate Workspace...",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "3_workspace", order: 3 }],
  run: duplicateWorkspace,
});
