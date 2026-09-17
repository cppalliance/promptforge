// Layout persistence: the dock's serialized layout plus the zone
// registry's placement memory, carried as one versioned envelope under
// the workspace bucket's "layout" key (the open .pfwork file, through the
// UI-state adapter). This module never touches storage itself: the
// composition root hands restoreLayout the preloaded value and
// startLayoutPersistence a writer, so the same code serves boot, the
// debounced live saves, and the Open and Save As paths that apply or
// write a workspace's layout. Writes are debounced off onDidLayoutChange.
// Only identity is stored - panels re-create through their registered
// factories on load. A restore that fails for any reason (a value that is
// not an envelope, a schema version bump, a fromJSON throw) clears the
// dock and reports failure so the caller boots the known-good default
// layout.

import type { DockviewApi, SerializedDockview } from "dockview";

import { DisposableStore, toDisposable, type IDisposable } from "../../base/lifecycle";
import { resetZones, restoreZoneState, serializeZoneState } from "./zones";

// v3: panels serialize their tabComponent; a v2 snapshot would restore
// the Workshop tree with a closable default tab.
export const LAYOUT_SCHEMA_VERSION = 3;

const SAVE_DEBOUNCE_MS = 250;

/**
 * The stored envelope: the dock layout plus the zone registry's fields.
 * `zones` and `overrides` stay opaque here; the zone registry validates
 * them on restore.
 */
export interface PersistedLayout {
  readonly version: typeof LAYOUT_SCHEMA_VERSION;
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

/** Validates a stored value as the envelope; null means fall back. */
function parsePersisted(value: unknown): PersistedLayout | null {
  if (!isRecord(value)) {
    return null;
  }
  if (value.version !== LAYOUT_SCHEMA_VERSION) {
    return null;
  }
  if (!isSerializedLayout(value.layout)) {
    return null;
  }
  return {
    version: LAYOUT_SCHEMA_VERSION,
    zones: value.zones,
    overrides: value.overrides,
    layout: value.layout,
  };
}

/**
 * Snapshots the live layout as the envelope the workspace bucket stores:
 * the debounced saver writes it, and Save As writes it once so the new
 * file carries the current arrangement.
 */
export function buildLayoutEnvelope(dock: DockviewApi): PersistedLayout {
  const state = serializeZoneState();
  return {
    version: LAYOUT_SCHEMA_VERSION,
    zones: state.zones,
    overrides: state.overrides,
    layout: dock.toJSON(),
  };
}

/**
 * Restores a stored envelope into the dock: panels re-create through
 * their registered factories, and the zone map and overrides come back
 * with them. Returns false when there is nothing to restore (null, or a
 * value that is not a current-version envelope) or anything fails - the
 * caller then builds the default layout onto a clean dock.
 */
export function restoreLayout(dock: DockviewApi, envelope: unknown): boolean {
  const persisted = parsePersisted(envelope);
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
 * debounced through `write`, which receives the fresh envelope. A throw
 * from the snapshot or the writer is logged, never propagated - the
 * in-memory layout stands and the next change tries again. Call once,
 * after the boot layout (restored or default) is in place. Returns the
 * disposable that stops persisting.
 */
export function startLayoutPersistence(
  dock: DockviewApi,
  write: (envelope: PersistedLayout) => void,
): IDisposable {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const store = new DisposableStore();
  store.add(
    dock.onDidLayoutChange(() => {
      if (timer !== null) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        timer = null;
        try {
          write(buildLayoutEnvelope(dock));
        } catch (error: unknown) {
          console.error("layout persistence: save failed:", error);
        }
      }, SAVE_DEBOUNCE_MS);
    }),
  );
  // Teardown order matters: the subscription dies first so no layout
  // change can re-arm the timer between the two steps; then any armed
  // save is cancelled unsaved.
  store.add(
    toDisposable(() => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    }),
  );
  return store;
}
