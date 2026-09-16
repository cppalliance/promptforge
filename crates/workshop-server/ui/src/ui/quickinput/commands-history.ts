// The commands history: the ids of the most recently accepted palette
// commands, most-recent-first, persisted to localStorage so the command
// palette's recency ordering survives a reload. The palette provider
// reads the list at every open and records each accepted command.
//
// The persisted value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as an empty list, never
// as a cast. Storage access itself can throw (denied access, quota), so
// every read and write is guarded and the store degrades to in-memory.
//
// Generic and DOM-free: nothing here may import from the app layers.

/** The localStorage key holding the persisted command id list. */
const STORAGE_KEY = "workshop.commandsHistory";

/** The most ids the store keeps; adding past the cap drops the oldest. */
const MAX_ENTRIES = 50;

/**
 * Narrows a persisted payload to a command id list: it must be a JSON
 * array, non-string and empty entries drop out, and duplicates collapse
 * keeping the first (most recent) occurrence. Anything else reads as no
 * history.
 */
function readIds(raw: string | null): readonly string[] {
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
  const ids: string[] = [];
  for (const item of parsed as readonly unknown[]) {
    if (typeof item === "string" && item !== "" && !ids.includes(item)) {
      ids.push(item);
    }
  }
  return ids;
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
 * The palette recency list. Mutations persist eagerly; a storage failure
 * leaves the in-memory list authoritative for the rest of the page
 * lifetime.
 */
export class CommandsHistory {
  private ids: readonly string[];

  constructor(
    private readonly storage: Storage | null = defaultStorage(),
    private readonly storageKey: string = STORAGE_KEY,
  ) {
    this.ids = readIds(this.readRaw());
  }

  /** The recorded command ids, most recent first. */
  get list(): readonly string[] {
    return this.ids;
  }

  /**
   * Records `id` as just run: it moves to the front, a duplicate
   * collapses, and the oldest entries drop past the cap. Empty ids are
   * ignored.
   */
  add(id: string): void {
    if (id === "") {
      return;
    }
    this.ids = [id, ...this.ids.filter((entry) => entry !== id)].slice(0, MAX_ENTRIES);
    this.persist();
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
      this.storage.setItem(this.storageKey, JSON.stringify(this.ids));
    } catch {
      // Quota or denied access: the in-memory list stays authoritative.
    }
  }
}

/** The shared history the running app's palette provider reads. */
export const commandsHistory = new CommandsHistory();
