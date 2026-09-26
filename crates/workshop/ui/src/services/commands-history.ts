// The commands-history service contract behind the command palette's
// recency ordering. The implementation (parts/quickinput/commands-history.ts)
// is user-persisted and self-registers with a default factory, but it stays
// in parts because the composition root binds it to the live adapter beside
// the other user-scoped stores; only the interface and the token live here,
// in the DOM-free services layer.

import { createServiceToken, type ServiceToken } from "./service-registry";

/** The palette recency list consumers resolve from the registry. */
export interface CommandsHistory {
  /** The recorded command ids, most recent first. */
  readonly list: readonly string[];
  /**
   * Records `id` as just run: it moves to the front, a duplicate
   * collapses, and the oldest entries drop past the cap. Empty ids are
   * ignored.
   */
  add(id: string): void;
}

/** The registry token for the commands-history singleton. */
export const COMMANDS_HISTORY: ServiceToken<CommandsHistory> =
  createServiceToken<CommandsHistory>("workshop.commandsHistory");
