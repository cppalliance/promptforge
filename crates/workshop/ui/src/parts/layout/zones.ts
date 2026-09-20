// The zone registry: the only module that talks to Dockview placement
// APIs. Zones are named tab banks - "left" holds the workspace tree,
// "main" holds document editors, "right" holds the agent session
// ("bottom" is reserved for later). Placement for a new panel resolves as
// the per-panel override recorded when the user last moved that panel,
// then the panel type's declared affinity from the panel registry.
//
// Zones never die from panel closes or drags. Dockview's removePanel
// defaults removeEmptyGroup: true, so a group's last panel leaving (a tab
// close, a drag onto another group) dissolves the group and the zone's
// space with it. This module keeps each zone's last known size, and when
// a zone's group dies inside a layout mutation, it re-creates the group
// as an empty dockview group at the zone's rebuilt position before the
// mutation is reported as settled, then restores the recorded sizes. An
// empty group is dockview's own idea of "nothing here": a blank tab strip
// over the default watermark. The next panel opened into the zone lands
// in that group. Restores and resets suppress the rebuild through
// withZoneRestore and the mutation kind.
//
// Dockview facts relied on, verified against dockview-core 8.3.1 (per
// package-lock.json) in dist/package/main.esm.mjs; test/zone-stability.mjs
// is the detecting test when an upgrade breaks one of them:
// - onWillMutateLayout / onDidMutateLayout bracket each top-level
//   structural change exactly once (nested calls join the outermost
//   transaction), so a compound drag that removes a group and adds a
//   panel elsewhere reports once, after it settled. A rebuild performed
//   inside onDidMutateLayout runs at depth zero and opens its own "add"
//   bracket, whose snapshot already sees the zone dead and rebuilds
//   nothing.
// - addGroup({ referenceGroup, direction }) creates a panel-less group
//   beside the reference group and answers it; fromJSON creates each grid
//   leaf's group before opening its views, so a leaf with views: [] comes
//   back as an empty group at its saved size, and toJSON serializes empty
//   leaves. Nothing beyond dockview's own layout needs to be persisted.
// - The default watermark is an empty div.dv-watermark: no text, no
//   controls.
// - noPanelsOverlay: "emptyGroup" (set in main.ts) keeps the dock's sole
//   remaining group when its last panel closes from the tab strip; the
//   dock-empty guard below covers the API removal path.
//
// The zone state itself (the group map and the placement overrides) lives
// in the ZoneStateService, resolved through the service registry; this
// module is the placement behavior over that state.

import "./zones.css";

import type { Direction, DockviewApi, IDockviewGroupPanel, IDockviewPanel } from "dockview";

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { DOCK, isPanelType, panelTypeEntry, type PanelType } from "../../services/panel-registry";
import { getService, registerService } from "../../services/service-registry";
import {
  ZONE_NAMES,
  ZoneStateService,
  ZONE_STATE,
  type ZoneName,
  type ZoneState,
} from "../../services/zone-state-service";

export { ZONE_NAMES } from "../../services/zone-state-service";
export type { PanelType } from "../../services/panel-registry";
export type { ZoneName, ZoneState } from "../../services/zone-state-service";

/** Parameters carried into a panel open; editor opens carry { path }. */
export type PanelParams = Record<string, unknown>;

let dock: DockviewApi | null = null;

/** The shared zone state: the group map and the placement overrides. */
function zoneState(): ZoneStateService {
  return getService(ZONE_STATE);
}

/** One zone group's last known dimensions, kept for resurrection. */
interface ZoneSize {
  readonly width: number;
  readonly height: number;
}

// The size memory: each zone group's last known dimensions, refreshed on
// every layout change. Session-scoped placement behavior (like the dock
// reference above), not application state; across a relaunch the sizes
// come back through dockview's own serialized grid.
const zoneSizes = new Map<ZoneName, ZoneSize>();

// The mutation snapshot: the zones that had a live group when the current
// top-level layout mutation opened, taken in onWillMutateLayout and
// consumed in onDidMutateLayout. Null outside a bracket and while a
// restore runs.
let liveBefore: readonly ZoneName[] | null = null;

// Set while a layout restore or reset runs: fromJSON and clear() remove
// every live group, and those removals must never rebuild anything.
let restoring = false;

/**
 * Runs `run` with the zone rebuild suppressed. Wraps the fromJSON in
 * layout-persistence's restoreLayout (the one caller) so layout reloads
 * and workspace switches never create groups.
 */
export function withZoneRestore<T>(run: () => T): T {
  restoring = true;
  try {
    return run();
  } finally {
    restoring = false;
  }
}

/** Records every zone group's current dimensions. */
function recordZoneSizes(): void {
  if (dock === null) {
    return;
  }
  for (const zone of ZONE_NAMES) {
    const group = liveGroup(zone);
    if (group !== undefined) {
      zoneSizes.set(zone, { width: group.api.width, height: group.api.height });
    }
  }
}

/**
 * Re-creates a zone's group after its last panel left: a fresh, empty
 * group at the zone's rebuilt position, sized back to the recorded
 * dimensions. The surviving zones' recorded sizes are re-asserted as
 * well - the splitview redistributes space on the removal and the
 * re-add, and the side zones must stay pixel-identical.
 */
function rebuildZone(zone: ZoneName): void {
  if (dock === null) {
    return;
  }
  const position = rebuildPosition(zone);
  if (position === undefined) {
    return;
  }
  const group = dock.addGroup(position);
  zoneState().setGroup(zone, group.id);
  const size = zoneSizes.get(zone);
  if (size !== undefined) {
    group.api.setSize(size);
  }
  for (const other of ZONE_NAMES) {
    if (other === zone) {
      continue;
    }
    const recorded = zoneSizes.get(other);
    const group = liveGroup(other);
    if (recorded !== undefined && group !== undefined) {
      group.api.setSize(recorded);
    }
  }
}

/**
 * The panel id for one open: editors key by path, new agent panels key by
 * their instance id, Run windows key by their instance id, and every
 * other panel kind is a singleton.
 */
export function panelIdFor(type: PanelType, params: PanelParams): string {
  if (type === "editor") {
    const path = params.path;
    if (typeof path === "string") {
      return `editor:${path}`;
    }
    // Untitled buffers key by their allocated serial, so each new buffer
    // is its own panel instead of reactivating the previous one.
    const untitled = params.untitled;
    if (typeof untitled === "number") {
      return `editor:untitled-${untitled}`;
    }
    return "editor:";
  }
  if (type === "agent" && typeof params.instance === "string") {
    return `agent:${params.instance}`;
  }
  if (type === "run" && typeof params.instance === "string") {
    return `run:${params.instance}`;
  }
  return type;
}

/** Recovers the panel type from a panel id built by panelIdFor. */
function panelTypeFromId(id: string): PanelType | null {
  const separator = id.indexOf(":");
  const name = separator === -1 ? id : id.slice(0, separator);
  return isPanelType(name) ? name : null;
}

/** The zone a panel currently lives in, by reverse group lookup. */
export function zoneOfPanel(panel: IDockviewPanel): ZoneName | undefined {
  return zoneState().zoneForGroupId(panel.group.id);
}

/**
 * Records where a panel now lives. Moving a panel writes an override;
 * moving it back to its type's affinity zone deletes the override.
 */
export function setZoneOverride(panelId: string, zone: ZoneName): void {
  const type = panelTypeFromId(panelId);
  const state = zoneState();
  if (type !== null && panelTypeEntry(type)?.defaultZone === zone) {
    state.clearOverride(panelId);
  } else {
    state.setOverride(panelId, zone);
  }
}

/**
 * Binds the registry to the dock. User drags (always possible: the
 * workbench is never locked) flow back into the override map through
 * onDidMovePanel. The dock itself registers as the DOCK service, so the
 * commands the feature directories register (save, close, toggle) resolve
 * the dock from the service registry instead of capturing it. Returns
 * the disposable owning that subscription.
 */
export function initZones(dockview: DockviewApi): IDisposable {
  dock = dockview;
  registerService(DOCK, () => dockview);
  const store = new DisposableStore();
  store.add(
    dockview.onDidMovePanel(({ panel, to }) => {
      const zone = zoneState().zoneForGroupId(to.id);
      if (zone !== undefined) {
        setZoneOverride(panel.id, zone);
      }
    }),
  );
  // The size memory refreshes on every layout change, so a rebuild
  // restores the dimensions the zone last had, not the ones it booted
  // with.
  store.add(dockview.onDidLayoutChange(() => recordZoneSizes()));
  // The rebuild boundary: snapshot which zones are live when a top-level
  // mutation opens, and once it has settled rebuild every zone that lost
  // its group in it. A zone that was not live before is never created
  // here - its first panel creates it through openInZone.
  store.add(
    dockview.onWillMutateLayout(() => {
      liveBefore = restoring ? null : ZONE_NAMES.filter((zone) => liveGroup(zone) !== undefined);
    }),
  );
  store.add(
    dockview.onDidMutateLayout((event) => {
      const before = liveBefore;
      liveBefore = null;
      // Restores and clears tear every group down on purpose. The
      // dock's last group standing is never rebuilt either: with no
      // survivors there is no zone layout left to preserve.
      if (
        before === null ||
        restoring ||
        event.kind === "load" ||
        event.kind === "clear" ||
        dock === null ||
        dock.groups.length === 0
      ) {
        return;
      }
      for (const zone of before) {
        if (liveGroup(zone) === undefined) {
          rebuildZone(zone);
        }
      }
    }),
  );
  return store;
}

/** The zone's group while it is alive; undefined once it has closed away. */
function liveGroup(zone: ZoneName): IDockviewGroupPanel | undefined {
  if (dock === null) {
    return undefined;
  }
  const id = zoneState().groupFor(zone);
  return id === undefined ? undefined : dock.getGroup(id);
}

/**
 * Hides or shows a zone's live group and answers the new visibility.
 * Hiding goes through the group's own setVisible, so its panels - and
 * the agent session's socket, for the right zone - survive; nothing is
 * removed. Answers undefined when the zone has no live group, leaving
 * the caller to open the zone's anchor panel instead.
 */
export function toggleZoneVisibility(zone: ZoneName): boolean | undefined {
  const group = liveGroup(zone);
  if (group === undefined) {
    return undefined;
  }
  const visible = !group.api.isVisible;
  group.api.setVisible(visible);
  return visible;
}

/**
 * A rebuilt zone's placement: a surviving group and the side to grow on.
 * The one shape both addGroup (AddGroupOptions) and addPanel's position
 * (AddPanelPositionOptions) accept; dockview spells the two direction
 * types differently, so neither alias fits both call sites.
 */
interface ZonePosition {
  readonly referenceGroup: string;
  readonly direction: Exclude<Direction, "within">;
}

/**
 * Placement for rebuilding a zone whose group is gone: the zone's own
 * side of the dock, anchored to a surviving group. "main" regrows beside
 * the left zone when it can, else beside the right zone. Returns undefined
 * when the dock has no groups at all - the first panel creates the first
 * group and becomes the zone by itself.
 */
function rebuildPosition(zone: ZoneName): ZonePosition | undefined {
  if (dock === null) {
    return undefined;
  }
  const groups = dock.groups;
  const first = groups[0];
  if (first === undefined) {
    return undefined;
  }
  if (zone === "main") {
    const left = liveGroup("left");
    if (left) {
      return { referenceGroup: left.id, direction: "right" };
    }
    const right = liveGroup("right");
    if (right) {
      return { referenceGroup: right.id, direction: "left" };
    }
    return { referenceGroup: first.id, direction: "right" };
  }
  return { referenceGroup: first.id, direction: zone };
}

/** The tab title for one open: editors take the file's base name. */
function titleFor(type: PanelType, params: PanelParams): string {
  if (type === "editor" || type === "run") {
    const path = params.path;
    if (typeof path === "string") {
      const name = path.split(/[\\/]/).filter(Boolean).pop();
      if (name !== undefined) {
        return type === "run" ? `Run: ${name}` : name;
      }
    }
  }
  return panelTypeEntry(type)?.title ?? type;
}

/**
 * Opens a panel in its zone: the user's recorded override first, then the
 * type's affinity. Reopening an already-open panel activates it. A live
 * zone group - empty or not - receives the panel; a zone whose group was
 * closed away is rebuilt on its side of the dock.
 */
export function openInZone(type: PanelType, params: PanelParams): IDockviewPanel {
  if (dock === null) {
    throw new Error("openInZone called before initZones.");
  }
  const entry = panelTypeEntry(type);
  if (entry === undefined) {
    throw new Error(`openInZone called with the unregistered panel type "${type}".`);
  }
  const id = panelIdFor(type, params);
  const existing = dock.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return existing;
  }
  const state = zoneState();
  const zone = state.overrideFor(id) ?? entry.defaultZone;
  const group = liveGroup(zone);
  const panel = dock.addPanel({
    id,
    component: entry.type,
    tabComponent: entry.tabComponent,
    title: titleFor(type, params),
    params,
    position: group ? { referenceGroup: group.id } : rebuildPosition(zone),
  });
  state.setGroup(zone, panel.group.id);
  return panel;
}

/** Snapshots the zone map and placement overrides for layout persistence. */
export function serializeZoneState(): ZoneState {
  return zoneState().serialize();
}

/**
 * Replaces the zone map and overrides from persisted state. Entries
 * naming unknown zones or carrying non-string values are dropped; stale
 * group ids self-heal because openInZone rebuilds a zone whose group no
 * longer exists.
 */
export function restoreZoneState(zones: unknown, overrides: unknown): void {
  zoneState().restore(zones, overrides);
}

/** Clears all zone state; the default-layout fallback starts from blank. */
export function resetZones(): void {
  zoneState().reset();
}
