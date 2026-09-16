// The recent-files store: the paths of the most recently opened files,
// most-recent-first, persisted to localStorage so the File > Open Recent
// menu and the quick-access "" provider survive a reload. The editor
// feature records every opened path; the workspace contribution reads
// the list for its dynamic menu rows and clears it from the Clear
// Recently Opened command.
//
// The persisted value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as an empty list, never
// as a cast. Storage access itself can throw (denied access, quota), so
// every read and write is guarded and the store degrades to in-memory.
//
// The service self-registers with a default factory, so any bundle that
// touches it gets the singleton without composition-root wiring.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { Emitter } from "../base/event";
import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, registerService } from "./service-registry";

/** The localStorage key holding the persisted path list. */
const STORAGE_KEY = "workshop.recentFiles";

/** The most paths the store keeps; adding past the cap drops the oldest. */
const MAX_ENTRIES = 50;

/**
 * Narrows a persisted payload to a path list: it must be a JSON array,
 * non-string and empty entries drop out, and duplicates collapse keeping
 * the first (most recent) occurrence. Anything else reads as no history.
 */
function readEntries(raw: string | null): readonly string[] {
  if (raw === null) {
    return [];
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) {
    return [];
  }
  const entries: string[] = [];
  for (const item of parsed as readonly unknown[]) {
    if (typeof item === "string" && item !== "" && !entries.includes(item)) {
      entries.push(item);
    }
  }
  return entries;
}

/** The page's localStorage, or null where none exists (a DOM-free host). */
function defaultStorage(): Storage | null {
  try {
    return typeof globalThis.localStorage === "undefined" ? null : globalThis.localStorage;
  } catch {
    return null;
  }
}

/**
 * The recent-files list. Mutations persist eagerly and fire onDidChange;
 * a storage failure leaves the in-memory list authoritative for the rest
 * of the page lifetime.
 */
export class RecentFilesStore implements IDisposable {
  private entries: readonly string[];
  private readonly changeEmitter = new Emitter<void>();

  /** Fires when the list changes; the Open Recent provider hooks it. */
  readonly onDidChange: Event<void> = this.changeEmitter.event;

  constructor(
    private readonly storage: Storage | null = defaultStorage(),
    private readonly storageKey: string = STORAGE_KEY,
  ) {
    this.entries = readEntries(this.readRaw());
  }

  /** The recorded paths, most recent first. */
  get list(): readonly string[] {
    return this.entries;
  }

  /**
   * Records `path` as just opened: it moves to the front, a duplicate
   * collapses, and the oldest entries drop past the cap. Empty paths are
   * ignored.
   */
  add(path: string): void {
    if (path === "") {
      return;
    }
    this.entries = [path, ...this.entries.filter((entry) => entry !== path)].slice(0, MAX_ENTRIES);
    this.persist();
    this.changeEmitter.fire();
  }

  /** Empties the list and persists the empty state. */
  clear(): void {
    if (this.entries.length === 0) {
      return;
    }
    this.entries = [];
    this.persist();
    this.changeEmitter.fire();
  }

  private readRaw(): string | null {
    if (this.storage === null) {
      return null;
    }
    try {
      return this.storage.getItem(this.storageKey);
    } catch {
      return null;
    }
  }

  private persist(): void {
    if (this.storage === null) {
      return;
    }
    try {
      this.storage.setItem(this.storageKey, JSON.stringify(this.entries));
    } catch {
      // Quota or denied access: the in-memory list stays authoritative.
    }
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

/** The registry token for the recent-files singleton. */
export const RECENT_FILES_STORE = createServiceToken<RecentFilesStore>("workshop.recentFiles");

// Self-registration: the default instance is shared by every consumer in
// the process. The composition root may re-register to rebind.
registerService(RECENT_FILES_STORE, () => new RecentFilesStore());
