// The Open Recent menu provider and the "" quick-access file provider
// (plan step 17). Both read the same two sources - the tree-state
// service's fetched listings (the granted roots cache under ROOTS_KEY)
// and the recent-files store - so File > Open Recent and Ctrl+P never
// disagree, and both cover only what is already there: no server route,
// no index. The menu provider is re-read at every open; the quick-access
// provider re-reads the stores at every getItems.
//
// The dynamic rows carry their own groups so they sort with the static
// Reopen Closed Editor (1_editor), More... (y_more), and Clear Recently
// Opened... (z_clear) rows: roots land in 2_roots and dispatch
// vscode.openFolder with the root path, recent files land in 3_files and
// dispatch vscode.open. When roots and history are both empty the
// provider answers no rows, which is what lets the menu widget drop the
// empty submenu.
//
// Every factory takes its stores as optional deps defaulting to the
// shared singletons, resolved at call time; tests inject their own.

import { baseName } from "../../base/paths";
import { Commands, type CommandRegistry } from "../../services/command-registry";
import type { MenuItem, MenuItemsProvider } from "../../services/menu-registry";
import { RECENT_FILES_STORE, type RecentFilesStore } from "../../services/recent-files-store";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { ROOTS_KEY, TREE_STATE, type TreeStateService } from "../../services/tree-state-service";
import { STATUS_BAR } from "../status/status-bar";
import type { QuickAccessProvider, QuickInputItem } from "../quickinput/quick-input";

/** The stores the providers read; tests inject their own. */
export interface RecentProviderDeps {
  readonly treeState?: TreeStateService;
  readonly recentFiles?: RecentFilesStore;
  readonly commands?: CommandRegistry;
}

/** Reports a rejected command run to the status bar, as the menu does. */
function reportCommandFailure(commandId: string, error: unknown): void {
  const statusBar = getServiceOrNull(STATUS_BAR);
  if (statusBar === null) {
    // No composition root (a widget test): keep the failure loud.
    console.error(`command '${commandId}' failed`, error);
    return;
  }
  const message = error instanceof Error ? error.message : String(error);
  statusBar.showLocal(`Could not run '${commandId}': ${message}`, "error");
}

/**
 * The dynamic rows of File > Open Recent: one row per granted root
 * (2_roots; re-focuses the tree on it) and one per recent file, most
 * recent first (3_files; opens an editor). Empty when both sources are
 * empty.
 */
export function createRecentMenuProvider(deps: RecentProviderDeps = {}): MenuItemsProvider {
  return (): readonly MenuItem[] => {
    const treeState = deps.treeState ?? getService(TREE_STATE);
    const recentFiles = deps.recentFiles ?? getService(RECENT_FILES_STORE);
    const rows: MenuItem[] = [];
    const roots = treeState.listing(ROOTS_KEY)?.entries ?? [];
    let order = 0;
    for (const root of roots) {
      if (root.kind !== "directory") {
        continue;
      }
      rows.push({ command: "vscode.openFolder", args: [root.path], title: root.name, group: "2_roots", order });
      order += 1;
    }
    order = 0;
    for (const path of recentFiles.list) {
      rows.push({ command: "vscode.open", args: [path], title: baseName(path), group: "3_files", order });
      order += 1;
    }
    return rows;
  };
}

/**
 * The "" quick-access provider: recent files first in recency order,
 * then every file in the tree's fetched listings, deduped by path. The
 * filter is a case-insensitive substring match on the base name or the
 * full path. Accepting dispatches vscode.open, the same command the
 * Open Recent rows carry.
 */
export function createFileQuickAccessProvider(deps: RecentProviderDeps = {}): QuickAccessProvider {
  return {
    getItems(filter: string): readonly QuickInputItem[] {
      const treeState = deps.treeState ?? getService(TREE_STATE);
      const recentFiles = deps.recentFiles ?? getService(RECENT_FILES_STORE);
      const commands = deps.commands ?? Commands;
      const needle = filter.trim().toLowerCase();
      const seen = new Set<string>();
      const rows: QuickInputItem[] = [];
      const addFile = (path: string): void => {
        if (seen.has(path)) {
          return;
        }
        const name = baseName(path);
        if (needle !== "" && !name.toLowerCase().includes(needle) && !path.toLowerCase().includes(needle)) {
          return;
        }
        seen.add(path);
        rows.push({
          label: name,
          description: path,
          accept: () => {
            void commands.execute("vscode.open", path).catch((error: unknown) => {
              reportCommandFailure("vscode.open", error);
            });
          },
        });
      };
      for (const path of recentFiles.list) {
        addFile(path);
      }
      for (const listing of treeState.cachedListings()) {
        for (const entry of listing.entries) {
          if (entry.kind === "file") {
            addFile(entry.path);
          }
        }
      }
      return rows;
    },
  };
}
