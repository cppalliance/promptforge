// The take registry's types: the immutable state, the inputs the reducer
// accepts, and the typed effects it hands its owner. MutableRegistry and
// Reduction live only inside one reduction; take-registry.ts re-exports
// the public subset.

import type { RealtimeEvent } from "../../services/realtime-event-decoder";

/** One immutable snapshot of the target-owned transcript insertion policy. */
export interface SttInsertionContext {
  /** The selected range in the target's coordinate space. */
  readonly range: {
    readonly start: number;
    readonly end: number;
  };
  /** The selected text, which a take restores when the target captured no `content`. */
  readonly original: string;
  /**
   * The selection in the target's own form, as plain data opaque to the
   * registry. A take restores it through the target's `restoreRange`, so
   * structure the text does not show, such as paragraph breaks and
   * mention chips, comes back intact.
   */
  readonly content?: unknown;
  /** The separator owned by this take, if appending requires one. */
  readonly compositionPrefix: "" | " ";
}

/** One transcript region owned by a Realtime audio take. */
export interface RegistryTake {
  readonly id: number;
  readonly generation: number;
  readonly from: number;
  readonly to: number;
  readonly original: string;
  /** The target's own form of the captured selection; undefined when it captured only text. */
  readonly originalContent: unknown;
  /** How many target positions the captured selection spans. */
  readonly originalWidth: number;
  readonly compositionPrefix: "" | " ";
  readonly itemId: string | null;
  readonly itemGeneration: number | null;
  readonly text: string;
  readonly deltaText: string;
  readonly hypothesisRevision: number | null;
  /** Whether the region holds text this take wrote, so later writes may patch it word by word. */
  readonly written: boolean;
  /** The length of the tentative tail that ends `text`; 0 when none is shown. */
  readonly tentativeLength: number;
  /** How many of the take's finished stable sentences have been announced. */
  readonly announcedSentences: number;
}

/** A span in the target's coordinate space. */
export interface SttTargetRange {
  readonly from: number;
  readonly to: number;
}

/** One wire request waiting for its client event identifier. */
export interface PendingWireRequest {
  readonly id: number;
  readonly generation: number;
  readonly command: "append" | "commit";
  readonly takeId: number;
}

/** One client event identifier bound to its owning take. */
export interface ClientEventBinding {
  readonly eventId: string;
  readonly generation: number;
  readonly takeId: number;
  readonly command: PendingWireRequest["command"];
}

/** One FIFO commit owner or a retired owner's acknowledgment tombstone. */
export interface CommitExpectation {
  readonly generation: number;
  readonly takeId: number | null;
  readonly itemId: string | null;
}

/** One generation-scoped item identifier that can no longer mutate state. */
export interface RetiredItem {
  readonly generation: number;
  readonly itemId: string;
}

/** All immutable state needed to assign and replace transcript regions. */
export interface TakeRegistry {
  readonly takes: readonly RegistryTake[];
  readonly awaitingCommit: readonly CommitExpectation[];
  readonly retiredItems: readonly RetiredItem[];
  readonly clientEvents: readonly ClientEventBinding[];
  readonly pendingWire: readonly PendingWireRequest[];
  readonly activeTakeId: number | null;
  readonly capture: "idle" | "recording" | "stopping";
  readonly stoppingTakeId: number | null;
  readonly connection: "ready" | "unavailable";
  readonly activeGeneration: number;
  readonly nextTakeId: number;
  readonly nextRequestId: number;
}

/** A user, capture, connection, or decoded server input to the registry. */
export type TakeRegistryInput =
  | { readonly type: "user.start"; readonly generation: number; readonly context: SttInsertionContext }
  | { readonly type: "user.stop"; readonly generation: number }
  | { readonly type: "user.discard"; readonly generation: number }
  | { readonly type: "capture.audio"; readonly generation: number; readonly chunk: ArrayBuffer }
  | {
      readonly type: "capture.stopped";
      readonly generation: number;
      readonly takeId: number;
      readonly ok: boolean;
    }
  | {
      readonly type: "wire.result";
      readonly generation: number;
      readonly requestId: number;
      readonly eventId: string | null;
    }
  | { readonly type: "server.event"; readonly generation: number; readonly event: RealtimeEvent }
  | { readonly type: "service.error"; readonly generation: number; readonly eventId: string | null }
  | { readonly type: "connection.lost"; readonly generation: number }
  | { readonly type: "connection.ready"; readonly generation: number };

/** A target edit the registry asks its UI owner to perform. */
export type TakeRegistryEditorEffect =
  | {
      readonly domain: "editor";
      readonly command: "replace";
      readonly from: number;
      readonly to: number;
      readonly text: string;
      /**
       * Whether the edit stays out of the target's undo history. Interim
       * writes and rollbacks are transient; only the write that lands a
       * take's text is not, so one dictation is one undo step.
       */
      readonly transient: boolean;
    }
  | {
      readonly domain: "editor";
      readonly command: "restore";
      readonly from: number;
      readonly to: number;
      /** The take's captured selection in the target's own form, written outside undo history. */
      readonly content: unknown;
    }
  | {
      readonly domain: "editor";
      readonly command: "caret";
      /** Where the collapsed caret goes: the end of the take just written. */
      readonly at: number;
    }
  | {
      readonly domain: "editor";
      readonly command: "tentative";
      readonly takeId: number;
      /** The take's shown tentative words, or null to clear its mark. */
      readonly range: SttTargetRange | null;
    }
  | {
      readonly domain: "editor";
      readonly command: "read-only";
      readonly readOnly: boolean;
    }
  | { readonly domain: "editor"; readonly command: "focus" };

/** A capture operation the registry asks its service owner to perform. */
export type TakeRegistryCaptureEffect =
  | {
      readonly domain: "capture";
      readonly command: "stop";
      readonly takeId: number;
      readonly generation: number;
    }
  | { readonly domain: "capture"; readonly command: "clear" };

/** A local status update emitted without server-authored wording. */
export type TakeRegistryStatusEffect =
  | {
      readonly domain: "status";
      readonly command: "recording";
      readonly recording: boolean;
    }
  | {
      readonly domain: "status";
      readonly command: "local";
      readonly label: string;
      readonly severity: "info" | "error";
    }
  | {
      readonly domain: "status";
      readonly command: "announce";
      /** Newly finished stable sentences, or the unannounced rest of a completed take. */
      readonly text: string;
    };

/** A wire operation the registry asks the Realtime owner to perform. */
export type TakeRegistryWireEffect =
  | {
      readonly domain: "wire";
      readonly command: "append";
      readonly generation: number;
      readonly requestId: number;
      readonly takeId: number;
      readonly chunk: ArrayBuffer;
    }
  | {
      readonly domain: "wire";
      readonly command: "commit";
      readonly generation: number;
      readonly requestId: number;
      readonly takeId: number;
    }
  | { readonly domain: "wire"; readonly command: "clear"; readonly generation: number };

/** A typed operation produced by a pure registry transition. */
export type TakeRegistryEffect =
  | TakeRegistryEditorEffect
  | TakeRegistryCaptureEffect
  | TakeRegistryStatusEffect
  | TakeRegistryWireEffect;

/** The next immutable registry state and operations for its owners. */
export interface TakeRegistryTransition {
  readonly state: TakeRegistry;
  readonly effects: readonly TakeRegistryEffect[];
}

/** A private writable clone used only during one pure reduction. */
export interface MutableRegistry {
  takes: RegistryTake[];
  awaitingCommit: CommitExpectation[];
  retiredItems: RetiredItem[];
  clientEvents: ClientEventBinding[];
  pendingWire: PendingWireRequest[];
  activeTakeId: number | null;
  capture: TakeRegistry["capture"];
  stoppingTakeId: number | null;
  connection: TakeRegistry["connection"];
  activeGeneration: number;
  nextTakeId: number;
  nextRequestId: number;
}

/** A private transition accumulator used only during one reduction. */
export interface Reduction {
  readonly state: MutableRegistry;
  readonly effects: TakeRegistryEffect[];
}
