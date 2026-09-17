// The commands history: the ids of the most recently accepted palette
// commands, most-recent-first, seeded from the UI-state adapter's user
// bucket at construction and written back through it on every change so
// the command palette's recency ordering survives a relaunch. The
// palette provider reads the list at every open and records each
// accepted command.
//
// The initial value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as an empty list, never
// as a cast. The writer is fire-and-forget: a write that throws is
// swallowed and the in-memory list stays authoritative.
//
// The history is a registry service: it self-registers under
// COMMANDS_HISTORY with a default factory (empty initial, no-op writer),
// and the composition root re-registers it bound to the live adapter
// before the palette first resolves it.
//
// Generic and DOM-free: the initial value and the writer are injected.

import { createServiceToken, registerService } from "../../services/service-registry";

/** The most ids the store keeps; adding past the cap drops the oldest. */
const MAX_ENTRIES = 50;

/** The writer the history hands the whole id list to after each change. */
export type CommandsHistoryWriter = (value: unknown) => void;

/**
 * Narrows a persisted payload to a command id list: it must be an array,
 * non-string and empty entries drop out, and duplicates collapse keeping
 * the first (most recent) occurrence. Anything else reads as no history.
 */
function readIds(initial: unknown): readonly string[] {
  if (!Array.isArray(initial)) {
    return [];
  }
  const ids: string[] = [];
  for (const item of initial as readonly unknown[]) {
    if (typeof item === "string" && item !== "" && !ids.includes(item)) {
      ids.push(item);
    }
  }
  return ids;
}

/**
 * The palette recency list. Mutations write through eagerly; a writer
 * failure leaves the in-memory list authoritative for the rest of the
 * page lifetime.
 */
export class CommandsHistory {
  private ids: readonly string[];

  /**
   * `initial` is the value the user bucket held at boot (any shape; see
   * readIds); `write` receives the whole list after each change.
   */
  constructor(
    initial: unknown = null,
    private readonly write: CommandsHistoryWriter = () => {},
  ) {
    this.ids = readIds(initial);
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

  private persist(): void {
    try {
      this.write(this.ids);
    } catch {
      // The adapter reports its own failures; a throwing writer leaves
      // the in-memory list authoritative.
    }
  }
}

/** The registry token for the commands-history singleton. */
export const COMMANDS_HISTORY = createServiceToken<CommandsHistory>("workshop.commandsHistory");

// Self-registration with an empty list and a no-op writer: a consumer
// that resolves the token before the composition root re-registers it
// bound to the live adapter gets a working, unpersisted history rather
// than a wrong instance cached for the page lifetime.
registerService(COMMANDS_HISTORY, () => new CommandsHistory());
