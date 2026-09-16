// The zone registry: the only module that talks to Dockview placement
// APIs. Zones are named tab banks - "left" holds the workspace tree,
// "main" holds document editors, "right" holds the agent session
// ("bottom" is reserved for later). Placement for a new panel resolves as
// the per-panel override recorded when the user last moved that panel,
// then the panel type's declared affinity from the panel registry. When
// every panel in a zone has been closed its Dockview group is gone; the
// next open into the zone rebuilds the group on its side of the dock.
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

/**
 * The panel id for one open: editors key by path, new agent panels key by
 * their instance id, and every other panel kind is a singleton.
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
  if (groups.length === 0) {
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
    return { referenceGroup: groups[0].id, direction: "right" };
  }
  const direction: Direction = zone;
  return { referenceGroup: groups[0].id, direction };
}

/** The tab title for one open: editors take the file's base name. */
function titleFor(type: PanelType, params: PanelParams): string {
  if (type === "editor") {
    const path = params.path;
    if (typeof path === "string") {
      const name = path.split(/[\\/]/).filter(Boolean).pop();
      if (name !== undefined) {
        return name;
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
