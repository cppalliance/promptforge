// The zone registry: the only module that talks to Dockview placement
// APIs. Zones are named tab banks - "left" holds the workspace tree,
// "main" holds document editors, "right" holds agent chats ("bottom" is
// reserved for later). Placement for a new panel resolves as the
// per-panel override recorded when the user last moved that panel, then
// the panel type's declared affinity from panel-types. When every panel
// in a zone has been closed its Dockview group is gone; the next open
// into the zone rebuilds the group on its side of the dock.

import type {
  AddPanelPositionOptions,
  Direction,
  DockviewApi,
  IDockviewGroupPanel,
  IDockviewPanel,
} from "dockview";

import { PANEL_TYPES, isPanelType, type PanelType } from "./panel-types";

export const ZONE_NAMES = ["left", "main", "right"] as const;
export type ZoneName = (typeof ZONE_NAMES)[number];

/** Parameters carried into a panel open; editor opens carry { path }. */
export type PanelParams = Record<string, unknown>;

let dock: DockviewApi | null = null;
// Zone name -> live Dockview group id. Entries go stale when the user
// closes a zone's last panel; openInZone rebuilds the group on demand.
const zoneGroups = new Map<ZoneName, string>();
// Panel id -> zone the user last moved it to. Survives panel close so a
// reopened panel returns to the user's chosen zone; step 15 persists it.
const zoneOverrides = new Map<string, ZoneName>();

/** The panel id for one open: singletons by type, editors keyed by path. */
export function panelIdFor(type: PanelType, params: PanelParams): string {
  if (type === "editor") {
    const path = params.path;
    return `editor:${typeof path === "string" ? path : ""}`;
  }
  return type;
}

/** Recovers the panel type from a panel id built by panelIdFor. */
function panelTypeFromId(id: string): PanelType | null {
  const separator = id.indexOf(":");
  const name = separator === -1 ? id : id.slice(0, separator);
  return isPanelType(name) ? name : null;
}

/** The zone owning a live group id, if the group is a known zone. */
function zoneForGroupId(groupId: string): ZoneName | undefined {
  for (const [zone, id] of zoneGroups) {
    if (id === groupId) {
      return zone;
    }
  }
  return undefined;
}

/** The zone a panel currently lives in, by reverse group lookup. */
export function zoneOfPanel(panel: IDockviewPanel): ZoneName | undefined {
  return zoneForGroupId(panel.group.id);
}

/**
 * Records where a panel now lives. Moving a panel writes an override;
 * moving it back to its type's affinity zone deletes the override.
 */
export function setZoneOverride(panelId: string, zone: ZoneName): void {
  const type = panelTypeFromId(panelId);
  if (type !== null && PANEL_TYPES[type].defaultZone === zone) {
    zoneOverrides.delete(panelId);
  } else {
    zoneOverrides.set(panelId, zone);
  }
}

/**
 * Binds the registry to the dock. User drags (possible once the layout
 * lock releases) flow back into the override map through onDidMovePanel.
 */
export function initZones(dockview: DockviewApi): void {
  dock = dockview;
  dock.onDidMovePanel(({ panel, to }) => {
    const zone = zoneForGroupId(to.id);
    if (zone !== undefined) {
      setZoneOverride(panel.id, zone);
    }
  });
}

/** The zone's group while it is alive; undefined once it has closed away. */
function liveGroup(zone: ZoneName): IDockviewGroupPanel | undefined {
  if (dock === null) {
    return undefined;
  }
  const id = zoneGroups.get(zone);
  return id === undefined ? undefined : dock.getGroup(id);
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
  return PANEL_TYPES[type].title;
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
  const id = panelIdFor(type, params);
  const existing = dock.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return existing;
  }
  const entry = PANEL_TYPES[type];
  const zone = zoneOverrides.get(id) ?? entry.defaultZone;
  const group = liveGroup(zone);
  const panel = dock.addPanel({
    id,
    component: entry.type,
    title: titleFor(type, params),
    params,
    position: group ? { referenceGroup: group.id } : rebuildPosition(zone),
  });
  zoneGroups.set(zone, panel.group.id);
  return panel;
}

/** Narrows a string to a declared zone name. */
function isZoneName(name: string): name is ZoneName {
  return (ZONE_NAMES as readonly string[]).includes(name);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The persisted placement state: live zone groups and user overrides. */
export interface ZoneState {
  readonly zones: Record<string, string>;
  readonly overrides: Record<string, string>;
}

/** Snapshots the zone map and placement overrides for layout persistence. */
export function serializeZoneState(): ZoneState {
  return {
    zones: Object.fromEntries(zoneGroups),
    overrides: Object.fromEntries(zoneOverrides),
  };
}

/**
 * Replaces the zone map and overrides from persisted state. Entries
 * naming unknown zones or carrying non-string values are dropped; stale
 * group ids self-heal because openInZone rebuilds a zone whose group no
 * longer exists.
 */
export function restoreZoneState(zones: unknown, overrides: unknown): void {
  zoneGroups.clear();
  zoneOverrides.clear();
  if (isRecord(zones)) {
    for (const [name, groupId] of Object.entries(zones)) {
      if (isZoneName(name) && typeof groupId === "string") {
        zoneGroups.set(name, groupId);
      }
    }
  }
  if (isRecord(overrides)) {
    for (const [panelId, zone] of Object.entries(overrides)) {
      if (typeof zone === "string" && isZoneName(zone)) {
        zoneOverrides.set(panelId, zone);
      }
    }
  }
}

/** Clears all zone state; the default-layout fallback starts from blank. */
export function resetZones(): void {
  zoneGroups.clear();
  zoneOverrides.clear();
}
