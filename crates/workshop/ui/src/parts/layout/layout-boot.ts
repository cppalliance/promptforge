// The layout boot decision, shared by the composition root (the preloaded
// workspace value at page load) and the Open Workspace action (the value
// re-read from the newly opened file): restore the stored envelope, or
// build the known-good default layout, then make sure the two anchor
// panels are present either way.

import type { DockviewApi } from "dockview";

import { restoreLayout } from "./layout-persistence";
import { openInZone, resetZones } from "./zones";

/**
 * Applies `envelope` to the dock, falling back to the default layout when
 * there is nothing valid to restore: the tree anchors the left zone first,
 * then the agent session opens right, and main stays empty until a
 * document opens. Panels re-create through their registered factories -
 * only identity is stored.
 *
 * The fallback starts from a blank dock. At boot the dock is already
 * empty; on Open Workspace it holds the previous workspace's panels, and
 * a file with no stored layout must still replace them with the default,
 * so boot and Open share one behavior.
 *
 * The workbench never boots without its anchors: a restored layout that
 * lost the Workshop tree (a stale snapshot from before the tree became
 * non-closable) or carries no agent-session panel gets them back. Both
 * panels are singletons, so re-opening an existing one only focuses it.
 */
export function applyLayoutOrDefault(dock: DockviewApi, envelope: unknown): void {
  if (!restoreLayout(dock, envelope)) {
    resetZones();
    dock.clear();
    const treePanel = openInZone("tree", {});
    treePanel.group.api.setSize({ width: 280 });
    openInZone("agent", {});
  }
  openInZone("tree", {});
  openInZone("agent", {});
}
