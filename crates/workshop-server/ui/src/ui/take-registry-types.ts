import type { RealtimeEvent } from "../services/realtime-event-decoder";
import type { SttInsertionContext } from "./stt";

/** One transcript region owned by a Realtime audio take. */
export interface RegistryTake {
  readonly id: number;
  readonly generation: number;
  readonly from: number;
  readonly to: number;
  readonly original: string;
  readonly compositionPrefix: "" | " ";
  readonly itemId: string | null;
  readonly itemGeneration: number | null;
  readonly text: string;
  readonly deltaText: string;
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
