// The zone registry: the only module that talks to Dockview placement
// APIs. Zones are named tab banks - "left" holds the workspace tree,
// "main" holds document editors, "right" holds the agent session
// ("bottom" is reserved for later). Placement for a new panel resolves as
// the per-panel override recorded when the user last moved that panel,
// then the panel type's declared affinity from the panel registry.
//
// Zones never die from panel closes. Dockview's removePanel defaults
// removeEmptyGroup: true, so a group's last panel leaving dissolves the
// group and the zone's space with it; every close path funnels through
// that removal. This module keeps each zone's last known size, and when
// a zone's group dies because its last real panel closed, it re-creates
// the group in the same tick at the zone's rebuilt position hosting the
// inert placeholder panel, then restores the recorded sizes. An explicit
// close of a group holding only its placeholder lets the zone die (an
// explicit close of real panels is indistinguishable from closing those
// panels - dockview's removeGroup funnels through the same per-panel
// path - and resurrects, the plan's accepted fallback). Placeholders are
// ordinary serialized panels, so the saved layout keeps the zones and a
// relaunch restores them. Restores and resets suppress resurrection
// through withZoneRestore.
//
// The zone state itself (the group map and the placement overrides) lives
// in the ZoneStateService, resolved through the service registry; this
// module is the placement behavior over that state.

import "./zones.css";

import type {
  AddPanelPositionOptions,
  Direction,
  DockviewApi,
  IDockviewGroupPanel,
  IDockviewPanel,
} from "dockview";

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
// reference above), not application state; the persisted sizes ride the
// layout envelope through the placeholder panels themselves.
const zoneSizes = new Map<ZoneName, ZoneSize>();

// The explicit-close heuristic: the last panel removal seen, consumed by
// the next group removal. A group that dies right after its final panel's
// removal event died because that panel closed. Dockview's removeGroup
// closes each panel through the same path a tab close takes, so an
// explicit close of a group holding real panels is indistinguishable
// from closing those panels and resurrects (the plan's accepted
// fallback); an explicit close of a group holding only its placeholder
// removes no real panel, and lets the zone die.
let lastPanelRemoval: {
  readonly groupId: string;
  readonly wasLast: boolean;
  readonly wasPlaceholder: boolean;
} | null = null;

// Set while a layout restore or reset runs: fromJSON and clear() remove
// every live group, and those removals must never spawn placeholders.
let restoring = false;

/**
 * Runs `run` with resurrection suppressed. Wraps the fromJSON in
 * layout-persistence's restoreLayout (the one caller) so layout reloads
 * and workspace switches never spawn placeholders.
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
 * Re-creates a zone's group after its last panel closed: a fresh group
 * at the zone's rebuilt position hosting the inert placeholder, sized
 * back to the recorded dimensions. The surviving zones' recorded sizes
 * are re-asserted as well - the splitview redistributes space on the
 * removal and the re-add, and the side zones must stay pixel-identical.
 */
function resurrectZone(zone: ZoneName): void {
  if (dock === null) {
    return;
  }
  const entry = panelTypeEntry("placeholder");
  if (entry === undefined) {
    return;
  }
  const panel = dock.addPanel({
    id: panelIdFor("placeholder", { zone }),
    component: entry.type,
    tabComponent: entry.tabComponent,
    title: entry.title,
    params: { zone },
    position: rebuildPosition(zone),
  });
  zoneState().setGroup(zone, panel.group.id);
  const size = zoneSizes.get(zone);
  if (size !== undefined) {
    panel.group.api.setSize(size);
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
 * their instance id, Run windows key by their instance id, placeholders
 * key by their zone, and every other panel kind is a singleton.
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
  if (type === "placeholder" && typeof params.zone === "string") {
    return `placeholder:${params.zone}`;
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
  // The size memory refreshes on every layout change, so a resurrection
  // restores the dimensions the zone last had, not the ones it booted
  // with.
  store.add(dockview.onDidLayoutChange(() => recordZoneSizes()));
  // The explicit-close heuristic's first half: remember each panel
  // removal (and whether it emptied its group) for the group removal
  // that may follow in the same close path.
  store.add(
    dockview.onDidRemovePanel((panel) => {
      lastPanelRemoval = {
        groupId: panel.group.id,
        wasLast: panel.group.panels.length === 0,
        wasPlaceholder: panelTypeFromId(panel.id) === "placeholder",
      };
    }),
  );
  store.add(
    dockview.onDidRemoveGroup((group) => {
      const removal = lastPanelRemoval;
      lastPanelRemoval = null;
      if (restoring) {
        return;
      }
      const zone = zoneState().zoneForGroupId(group.id);
      if (zone === undefined) {
        return;
      }
      // Let the zone die when the group did not die of its last real
      // panel closing: an empty group's explicit removal, or the
      // explicit close of a group holding only its placeholder.
      if (
        removal === null ||
        removal.groupId !== group.id ||
        !removal.wasLast ||
        removal.wasPlaceholder
      ) {
        return;
      }
      // The dock's last group standing is never resurrected: with no
      // survivors there is no zone layout left to preserve.
      if (dock === null || dock.groups.length === 0) {
        return;
      }
      resurrectZone(zone);
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
 * Placement for rebuilding a zone whose group is gone: the zone's own
 * side of the dock, anchored to a surviving group. "main" regrows beside
 * the left zone when it can, else beside the right zone. Returns undefined
 * when the dock has no groups at all - the first panel creates the first
 * group and becomes the zone by itself.
 */
function rebuildPosition(zone: ZoneName): AddPanelPositionOptions | undefined {
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
  const direction: Direction = zone;
  return { referenceGroup: first.id, direction };
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
 * type's affinity. Reopening an already-open panel activates it. A zone
 * whose group was closed away is rebuilt on its side of the dock.
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
  // A zone holding only its placeholder hands the group over: the real
  // panel joins first (so the group never empties), then the placeholder
  // leaves. The zone keeps its space and its recorded size.
  if (group !== undefined && type !== "placeholder") {
    for (const occupant of [...group.panels]) {
      if (occupant !== panel && panelTypeFromId(occupant.id) === "placeholder") {
        dock.removePanel(occupant);
      }
    }
  }
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
