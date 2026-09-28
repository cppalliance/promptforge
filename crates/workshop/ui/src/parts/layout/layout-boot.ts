// The layout boot decision, shared by the composition root (the preloaded
// workspace value at page load) and the Open Workspace action (the value
// re-read from the newly opened file): restore the stored envelope, or
// seed the default layout, then make sure the anchor panels are present
// either way. The default layout and its anchors are product policy,
// resolved from LAYOUT_POLICY; this module names no panel type.

import type { DockviewApi } from "dockview";

import { getService } from "@workshop/platform/service-registry";
import { LAYOUT_POLICY } from "../../services/layout-policy";
import { restoreLayout } from "./layout-persistence";
import { openInZone, resetZones } from "./zones";

/**
 * Applies `envelope` to the dock, falling back to the registered policy's
 * default layout when there is nothing valid to restore. Panels
 * re-create through their registered factories - only identity is
 * stored.
 *
 * The fallback starts from a blank dock. At boot the dock is already
 * empty; on Open Workspace it holds the previous workspace's panels, and
 * a file with no stored layout must still replace them with the default,
 * so boot and Open share one behavior.
 *
 * The workbench never boots without the policy's anchors: a restored
 * layout that lost one (a stale snapshot, or an anchor closed away before
 * the save) gets it back. Anchors are singletons, so re-opening an
 * existing one only focuses it.
 */
export function applyLayoutOrDefault(dock: DockviewApi, envelope: unknown): void {
  const policy = getService(LAYOUT_POLICY);
  if (!restoreLayout(dock, envelope)) {
    resetZones();
    dock.clear();
    policy.seed();
  }
  for (const anchor of policy.anchors) {
    openInZone(anchor, {});
  }
}
