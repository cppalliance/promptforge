// The zone-state service: the workbench's zone group map (which Dockview
// group currently hosts each named zone) and the per-panel placement
// overrides recorded when the user drags a panel to another zone. This
// state used to sit at module scope in zones.ts; as a registry-held
// service it is named, observable through onDidChange, and shared by
// every consumer through the service registry rather than through imports
// of one module's internals. The service self-registers with a default
// factory, so any bundle that touches it gets the singleton without
// composition-root wiring.
//
// The service owns state only: Dockview placement APIs stay in zones.ts.

import { Emitter } from "../base/event";
import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, registerService } from "./service-registry";

export const ZONE_NAMES = ["left", "main", "right"] as const;
export type ZoneName = (typeof ZONE_NAMES)[number];

/** The persisted placement state: live zone groups and user overrides. */
export interface ZoneState {
  readonly zones: Record<string, string>;
  readonly overrides: Record<string, string>;
}

/** Narrows a string to a declared zone name. */
export function isZoneName(name: string): name is ZoneName {
  return (ZONE_NAMES as readonly string[]).includes(name);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The zone group map and placement overrides. Zone name -> live Dockview
 * group id entries go stale when the user closes a zone's last panel;
 * zones.ts rebuilds the group on demand. Panel id -> zone overrides
 * survive panel close, so a reopened panel returns to the user's chosen
 * zone.
 */
export class ZoneStateService implements IDisposable {
  private readonly groups = new Map<ZoneName, string>();
  private readonly overrides = new Map<string, ZoneName>();
  private readonly changeEmitter = new Emitter<void>();

  /** Fires when the placement overrides change; layout persistence hooks it. */
  readonly onDidChange: Event<void> = this.changeEmitter.event;

  /** The zone's live group id, or undefined once the group has closed away. */
  groupFor(zone: ZoneName): string | undefined {
    return this.groups.get(zone);
  }

  /** Records which group currently hosts a zone. */
  setGroup(zone: ZoneName, groupId: string): void {
    this.groups.set(zone, groupId);
  }

  /** The zone owning a live group id, if the group is a known zone. */
  zoneForGroupId(groupId: string): ZoneName | undefined {
    for (const [zone, id] of this.groups) {
      if (id === groupId) {
        return zone;
      }
    }
    return undefined;
  }

  /** The zone the user last moved a panel to, if overridden. */
  overrideFor(panelId: string): ZoneName | undefined {
    return this.overrides.get(panelId);
  }

  /** Records a panel's user-chosen zone. */
  setOverride(panelId: string, zone: ZoneName): void {
    this.overrides.set(panelId, zone);
    this.changeEmitter.fire();
  }

  /** Drops a panel's override, returning it to its type's affinity zone. */
  clearOverride(panelId: string): void {
    if (this.overrides.delete(panelId)) {
      this.changeEmitter.fire();
    }
  }

  /** Snapshots the zone map and placement overrides for layout persistence. */
  serialize(): ZoneState {
    return {
      zones: Object.fromEntries(this.groups),
      overrides: Object.fromEntries(this.overrides),
    };
  }

  /**
   * Replaces the zone map and overrides from persisted state. Entries
   * naming unknown zones or carrying non-string values are dropped; stale
   * group ids self-heal because zones.ts rebuilds a zone whose group no
   * longer exists.
   */
  restore(zones: unknown, overrides: unknown): void {
    this.groups.clear();
    this.overrides.clear();
    if (isRecord(zones)) {
      for (const [name, groupId] of Object.entries(zones)) {
        if (isZoneName(name) && typeof groupId === "string") {
          this.groups.set(name, groupId);
        }
      }
    }
    if (isRecord(overrides)) {
      for (const [panelId, zone] of Object.entries(overrides)) {
        if (typeof zone === "string" && isZoneName(zone)) {
          this.overrides.set(panelId, zone);
        }
      }
    }
  }

  /** Clears all zone state; the default-layout fallback starts from blank. */
  reset(): void {
    this.groups.clear();
    this.overrides.clear();
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

/** The registry token for the zone-state singleton. */
export const ZONE_STATE = createServiceToken<ZoneStateService>("workshop.zoneState");

// Self-registration: the default instance is shared by every consumer in
// the process. The composition root may re-register to rebind.
registerService(ZONE_STATE, () => new ZoneStateService());
