// Layout persistence: the dock's serialized layout plus the zone
// registry's placement memory, written to localStorage under one
// versioned key. Writes are debounced off onDidLayoutChange. Only
// identity is stored - panels re-create through their registered
// factories on load. A restore that fails for any reason (bad JSON, a
// schema version bump, a fromJSON throw) clears the dock and reports
// failure so the caller boots the known-good default layout.

import type { DockviewApi, SerializedDockview } from "dockview";

import { resetZones, restoreZoneState, serializeZoneState } from "./zones";

export const LAYOUT_STORAGE_KEY = "promptforge.workshop.layout";
// v3: panels serialize their tabComponent; a v2 snapshot would restore
// the Workshop tree with a closable default tab.
export const LAYOUT_SCHEMA_VERSION = 3;

const SAVE_DEBOUNCE_MS = 250;

/** The storage envelope after validation: the dock layout plus our fields. */
interface PersistedLayout {
  readonly zones: unknown;
  readonly overrides: unknown;
  readonly layout: SerializedDockview;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Structural check only: fromJSON performs the deep validation, inside
 * the caller's try/catch, so a corrupt layout can never half-load.
 */
function isSerializedLayout(value: unknown): value is SerializedDockview {
  return isRecord(value) && isRecord(value.grid);
}

/** Parses and validates the storage envelope; null means fall back. */
function parsePersisted(raw: string): PersistedLayout | null {
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isRecord(body)) {
    return null;
  }
  if (body.version !== LAYOUT_SCHEMA_VERSION) {
    return null;
  }
  if (!isSerializedLayout(body.layout)) {
    return null;
  }
  return {
    zones: body.zones,
    overrides: body.overrides,
    layout: body.layout,
  };
}

/** Writes the current layout envelope. Failures are logged, never thrown. */
export function persistLayout(dock: DockviewApi): void {
  try {
    const state = serializeZoneState();
    const envelope = {
      version: LAYOUT_SCHEMA_VERSION,
      zones: state.zones,
      overrides: state.overrides,
      layout: dock.toJSON(),
    };
    localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(envelope));
  } catch (error: unknown) {
    console.error("layout persistence: save failed:", error);
  }
}

/**
 * Restores the persisted layout into the dock: panels re-create through
 * their registered factories, and the zone map and overrides come back
 * with them. Returns false when there is nothing to restore or anything
 * fails - the caller then builds the default layout onto a clean dock.
 */
export function restoreLayout(dock: DockviewApi): boolean {
  let raw: string | null;
  try {
    raw = localStorage.getItem(LAYOUT_STORAGE_KEY);
  } catch {
    return false;
  }
  if (raw === null) {
    return false;
  }
  const persisted = parsePersisted(raw);
  if (persisted === null) {
    return false;
  }
  try {
    dock.fromJSON(persisted.layout);
  } catch (error: unknown) {
    console.error("layout persistence: restore failed, falling back to defaults:", error);
    resetZones();
    try {
      dock.clear();
    } catch {
      // Dockview already cleaned up after the failed fromJSON.
    }
    return false;
  }
  restoreZoneState(persisted.zones, persisted.overrides);
  return true;
}

/**
 * Starts persistence for the running session: layout changes save
 * debounced. Call once, after the boot layout (restored or default) is
 * in place.
 */
export function startLayoutPersistence(dock: DockviewApi): void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  dock.onDidLayoutChange(() => {
    if (timer !== null) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = null;
      persistLayout(dock);
    }, SAVE_DEBOUNCE_MS);
  });
}
