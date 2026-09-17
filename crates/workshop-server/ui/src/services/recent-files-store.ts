// The recent-files store: the paths of the most recently opened files,
// most-recent-first, seeded from the UI-state adapter's user bucket at
// construction and written back through it on every change so the
// File > Open Recent menu and the quick-access "" provider survive a
// relaunch. The editor feature records every opened path; the workspace
// contribution reads the list for its dynamic menu rows and clears it
// from the Clear Recently Opened command.
//
// The initial value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as an empty list, never
// as a cast. The writer is fire-and-forget: a write that throws is
// swallowed and the in-memory list stays authoritative.
//
// The service self-registers with a default factory (empty initial,
// no-op writer), so any bundle that touches it gets a working singleton;
// the composition root re-registers it bound to the live adapter before
// the first consumer resolves it.
//
// Generic and DOM-free: the initial value and the writer are injected,
// and nothing here may import from the app layers.

import { Emitter } from "../base/event";
import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, registerService } from "./service-registry";

/** The most paths the store keeps; adding past the cap drops the oldest. */
const MAX_ENTRIES = 50;

/** The writer the store hands the whole path list to after each change. */
export type RecentFilesWriter = (value: unknown) => void;

/**
 * Narrows a persisted payload to a path list: it must be an array,
 * non-string and empty entries drop out, and duplicates collapse keeping
 * the first (most recent) occurrence. Anything else reads as no history.
 */
function readEntries(initial: unknown): readonly string[] {
  if (!Array.isArray(initial)) {
    return [];
  }
  const entries: string[] = [];
  for (const item of initial as readonly unknown[]) {
    if (typeof item === "string" && item !== "" && !entries.includes(item)) {
      entries.push(item);
    }
  }
  return entries;
}

/**
 * The recent-files list. Mutations write through eagerly and fire
 * onDidChange; a writer failure leaves the in-memory list authoritative
 * for the rest of the page lifetime.
 */
export class RecentFilesStore implements IDisposable {
  private entries: readonly string[];
  private readonly changeEmitter = new Emitter<void>();

  /** Fires when the list changes; the Open Recent provider hooks it. */
  readonly onDidChange: Event<void> = this.changeEmitter.event;

  /**
   * `initial` is the value the user bucket held at boot (any shape; see
   * readEntries); `write` receives the whole list after each change.
   */
  constructor(
    initial: unknown = null,
    private readonly write: RecentFilesWriter = () => {},
  ) {
    this.entries = readEntries(initial);
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

  /** Empties the list and writes the empty state. */
  clear(): void {
    if (this.entries.length === 0) {
      return;
    }
    this.entries = [];
    this.persist();
    this.changeEmitter.fire();
  }

  private persist(): void {
    try {
      this.write(this.entries);
    } catch {
      // The adapter reports its own failures; a throwing writer leaves
      // the in-memory list authoritative.
    }
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

/** The registry token for the recent-files singleton. */
export const RECENT_FILES_STORE = createServiceToken<RecentFilesStore>("workshop.recentFiles");

// Self-registration with an empty list and a no-op writer: a consumer
// that resolves the token before the composition root re-registers it
// bound to the live adapter gets a working, unpersisted list rather than
// a wrong instance cached for the page lifetime.
registerService(RECENT_FILES_STORE, () => new RecentFilesStore());
