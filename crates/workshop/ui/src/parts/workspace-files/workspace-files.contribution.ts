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
// the existing promptforge:workspace-changed event (marked roots-current:
// open dropped the roots before re-creating the tree, save as and
// duplicate keep the grants) so the window title and the tree refresh
// without a second roots fetch, emits the promptforge:workspace-opened
// Tauri event so the shell can fetch and apply the file's window
// geometry (native geometry is the shell's to apply, never the page's),
// and records the file in the recent-files store. Failures paint the
// status bar, exactly as the other file actions do; success is silent,
// the refreshed tree being its own confirmation; a cancelled picker is
// a no-op.
//
// The workspace-scoped UI state (plan step 13) follows the switch too. The
// .pfwork file holds the dock layout, the tree's expanded folders, and
// the closed-editor stack in its workspace bucket. Open pulls the file's
// bucket through the UI-state adapter and applies all three to the live
// stores with writes suppressed, so a store that writes synchronously on
// replace never echoes the values straight back to the file they came
// from; the layout saver, whose change event arrives on a microtask and
// whose write is debounced, drops a restore's echo itself. Save As writes the three live
// values once each into the new file, which the server created with
// grants only; the page is the single writer of that fact. Duplicate
// copies the file wholesale, state included, and touches nothing here.

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import { errorText, type Result } from "../../services/error-catalog";
import { isRecord } from "../../services/json-request";
import { MenuId } from "../../services/menu-registry";
import { DOCK } from "../../services/panel-registry";
import { RECENT_FILES_STORE } from "../../services/recent-files-store";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { TREE_STATE } from "../../services/tree-state-service";
import { UI_STORAGE } from "../../services/ui-storage";
import {
  currentWorkspaceFile,
  duplicateWorkspaceFile,
  openWorkspaceFile,
  saveWorkspaceFileAs,
  type WorkspaceFileResponse,
} from "../../services/workspace-file-client";
import { CLOSED_EDITORS } from "../editor/closed-editors";
import { applyLayoutOrDefault } from "../layout/layout-boot";
import { buildLayoutEnvelope } from "../layout/layout-persistence";
import { STATUS_BAR } from "../status/status-bar";
import { WORKSPACE_CHANGED_EVENT, type WorkspaceChangedDetail } from "../workspace/workspace-drops";

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
 * workspace-changed event for its other listeners (the window title,
 * the tree panel), the shell event, and the recent entry. Runs only
 * after the server has answered success, so nothing here can undo it.
 * The tree invalidation is not its job: Open drops the roots before it
 * applies the file's state (applyOpenedWorkspaceState), and Save As and
 * Duplicate keep the grants, so the event says the roots are current
 * and the tree keeps the listing it holds or the load it started.
 */
async function announceSwitched(path: string): Promise<void> {
  const detail: WorkspaceChangedDetail = { rootsCurrent: true };
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT, { detail }));
  await announceOpened(path);
  getService(RECENT_FILES_STORE).add(path);
}

/** The string entries of `value[key]` when it is an array; empty otherwise. */
function stringsUnder(value: unknown, key: string): string[] {
  if (!isRecord(value)) {
    return [];
  }
  const items: unknown = value[key];
  return Array.isArray(items) ? items.filter((item): item is string => typeof item === "string") : [];
}

/** The expanded paths of a stored `tree` value, `{ expanded: [...] }`. */
function expandedFrom(value: unknown): string[] {
  return stringsUnder(value, "expanded");
}

/** The paths of a stored `closed_editors` value, `{ paths: [...] }`. */
function pathsFrom(value: unknown): string[] {
  return stringsUnder(value, "paths");
}

/**
 * Pulls the newly opened file's workspace bucket and applies it to the
 * live stores: the dock layout (or the default when the file has none),
 * the tree's expanded folders, and the closed-editor stack. The roots
 * listing goes first: it belongs to the previous workspace, and the
 * layout apply re-creates the tree panel, whose init loads the roots
 * from the service; dropped beforehand, that load fetches the new
 * workspace's roots once and the tree never paints the old ones, and
 * replaceExpanded then re-renders from the same load. Writes stay
 * suppressed throughout, so no store's synchronous reaction to its
 * replace echoes the file's own values back into it; the layout saver's
 * deferred reaction is dropped by the saver (see startLayoutPersistence),
 * because Dockview delivers it after this span ends. Runs after the server
 * has committed the open; a failure inside the apply is logged and the
 * page continues on whatever applied, because the open already happened.
 */
async function applyOpenedWorkspaceState(): Promise<void> {
  const storage = getService(UI_STORAGE);
  await storage.reloadWorkspace();
  try {
    const dock = getService(DOCK);
    const tree = getService(TREE_STATE);
    const closed = getService(CLOSED_EDITORS);
    storage.suppressWrites(() => {
      tree.invalidateRoots();
      applyLayoutOrDefault(dock, storage.get("workspace", "layout"));
      tree.replaceExpanded(expandedFrom(storage.get("workspace", "tree")));
      closed.replaceClosedEditors(pathsFrom(storage.get("workspace", "closed_editors")));
    });
  } catch (error) {
    console.error(`workspace state apply: ${errorText(error)}`);
  }
}

/**
 * Writes the live arrangement into the workspace bucket once each, so a
 * file the server just created from the grants alone gets the
 * current layout, expanded folders, and closed stack too.
 */
function writeLiveWorkspaceState(): void {
  const storage = getService(UI_STORAGE);
  storage.set("workspace", "layout", buildLayoutEnvelope(getService(DOCK)));
  storage.set("workspace", "tree", { expanded: [...getService(TREE_STATE).expandedPaths] });
  storage.set("workspace", "closed_editors", getService(CLOSED_EDITORS).snapshot());
}

/**
 * The native picker filtered to .pfwork. Answers the picked path, or
 * null when the picker was cancelled (it answers a non-string then).
 */
async function pickWorkspaceFile(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ title: "Open Workspace from File", filters: WORKSPACE_FILTERS });
  return typeof picked === "string" ? picked : null;
}

/**
 * Open Workspace from File...: the open on the server, the file's UI
 * state applied to the live stores, then the invalidation, the shell
 * event, and the recent entry. With a string `path` argument (an Open
 * Recent row or a Ctrl+P hit) the picker is skipped and the argument is
 * the file; otherwise the native picker filtered to .pfwork supplies it.
 * A cancelled picker is a no-op; a refusal reports and changes nothing
 * on the page.
 */
async function openWorkspaceFromFile(path?: unknown): Promise<void> {
  const target = typeof path === "string" ? path : await pickWorkspaceFile();
  if (target === null) {
    return;
  }
  try {
    await openWorkspaceFile(target);
  } catch (error) {
    reportError(`Could not open ${target}: ${errorText(error)}`);
    return;
  }
  await applyOpenedWorkspaceState();
  await announceSwitched(target);
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
 * the switch posted, `afterSwitch` run against the new file, then the
 * page's announcement. A cancelled picker answers null and is a no-op;
 * a refusal reports and changes nothing.
 */
async function switchThroughSavePicker(
  title: string,
  verb: string,
  post: (path: string) => Promise<WorkspaceFileResponse>,
  afterSwitch: () => void = () => {},
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
  afterSwitch();
  await announceSwitched(target);
}

/**
 * Save Workspace As...: a new file holding the current grants, the switch
 * onto it, then the live UI state written into it (the server's save_as
 * writes grants only).
 */
function saveWorkspaceAs(): Promise<void> {
  return switchThroughSavePicker(
    "Save Workspace As",
    "save workspace as",
    saveWorkspaceFileAs,
    writeLiveWorkspaceState,
  );
}

/**
 * Duplicate Workspace...: a copy of the current file and its siblings,
 * then the switch onto it. The copy already holds the file's UI state,
 * so nothing is written here.
 */
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
