// The server-owned workbench state. The server pushes complete snapshots
// over the socket; this service holds the last one and fans out changes.
// App-aware and DOM-free: the composition root constructs one instance,
// feeds it from the socket's onWorkbench handler, and hands it to the
// Model menu and the chat-gating hook through their constructors.

import { Emitter, type Event } from "../base/event";
import { Disposable } from "../base/lifecycle";
import type { WorkbenchFrame } from "./protocol";

/**
 * One held workbench snapshot: the wire frame's fields under UI naming.
 * Absent options are `null`, never missing fields - every snapshot is the
 * complete menu state, exactly as the server pushed it.
 */
export interface WorkbenchSnapshot {
  /** Every profile the gateway can load, by name. */
  readonly profiles: readonly string[];
  /** The active profile's name, or null when unknown. */
  readonly active: string | null;
  /** The profile a switch is loading, or null when none is in flight. */
  readonly switching: string | null;
  /** The selected model's id, or null when no model is selected. */
  readonly selected: string | null;
  /** Whether the server considers chat usable; the UI never derives it. */
  readonly chatReady: boolean;
}

// The state before the first push: nothing known, chat gated off.
const EMPTY_SNAPSHOT: WorkbenchSnapshot = {
  profiles: [],
  active: null,
  switching: null,
  selected: null,
  chatReady: false,
};

/**
 * Holds the last workbench snapshot the server pushed and notifies
 * subscribers on every new one. Pure state: applying a snapshot never
 * sends anything back on the socket, so a push can never echo.
 */
export class WorkbenchService extends Disposable {
  private _snapshot: WorkbenchSnapshot = EMPTY_SNAPSHOT;

  private readonly _onDidChangeSnapshot = this._register(new Emitter<WorkbenchSnapshot>());
  /** Fires with the new snapshot after every applySnapshot call. */
  readonly onDidChangeSnapshot: Event<WorkbenchSnapshot> = this._onDidChangeSnapshot.event;

  /** The last snapshot applied, or the empty pre-boot state. */
  get snapshot(): WorkbenchSnapshot {
    return this._snapshot;
  }

  /**
   * Records one pushed workbench frame and notifies subscribers. Every
   * push is fanned out even when the fields are unchanged: snapshots are
   * complete and authoritative, so subscribers re-render rather than diff.
   */
  applySnapshot(frame: WorkbenchFrame): void {
    this._snapshot = {
      profiles: frame.profiles,
      active: frame.active,
      switching: frame.switching,
      selected: frame.selected,
      chatReady: frame.chat_ready,
    };
    this._onDidChangeSnapshot.fire(this._snapshot);
  }
}
