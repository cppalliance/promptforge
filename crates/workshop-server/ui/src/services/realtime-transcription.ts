import { Emitter, type Event as ServiceEvent } from "../base/event";
import { Disposable } from "../base/lifecycle";
import {
  decodeRealtimeEvent,
  type RealtimeEvent,
} from "./realtime-event-decoder";

const HYPOTHESIS_INCLUDE = "item.input_audio_transcription.hypothesis";
const RECONNECT_INITIAL_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

/** Readiness of the browser's Realtime transcription connection. */
export type RealtimeTranscriptionState = "connecting" | "ready" | "unavailable";

/** One immutable WebSocket identity assigned in construction order. */
export type RealtimeSocketGeneration = number;

/** A connection-state change retaining its originating socket generation. */
export interface RealtimeTranscriptionStateEvent {
  readonly state: RealtimeTranscriptionState;
  readonly generation: RealtimeSocketGeneration;
}

/** A decoded event retaining its originating socket generation. */
export interface RealtimeTranscriptionEvent {
  readonly event: RealtimeEvent;
  readonly generation: RealtimeSocketGeneration;
}

/** A recoverable transport or server-event decoding failure. */
export interface RealtimeTranscriptionError {
  readonly code: string;
  readonly scope: "connection" | "session";
  readonly eventId: string | null;
  readonly recoverable: true;
  readonly generation: RealtimeSocketGeneration;
}

/** A client send result retaining the socket generation used for the attempt. */
export interface RealtimeTranscriptionResult {
  readonly generation: RealtimeSocketGeneration;
  readonly eventId: string | null;
}

/** The WebSocket surface used by the DOM-free Realtime service. */
export interface RealtimeSocket {
  readonly readyState: number;
  addEventListener?(
    type: "open" | "message" | "error" | "close",
    listener: (event: unknown) => void,
    options?: AddEventListenerOptions,
  ): void;
  onmessage?: ((event: MessageEvent<unknown>) => void) | null;
  onerror?: ((event: globalThis.Event) => void) | null;
  onclose?: ((event: CloseEvent) => void) | null;
  send(data: string): void;
  close(): void;
}

/** Injectable construction options for Realtime transcription. */
export interface RealtimeTranscriptionOptions {
  readonly prompt?: string;
  readonly eventId?: () => string;
  readonly socket?: (url: string) => RealtimeSocket;
}

function defaultEventId(): string {
  return `client_${crypto.randomUUID()}`;
}

function defaultSocket(url: string): RealtimeSocket {
  return new WebSocket(url);
}

function socketUrl(): string {
  if (typeof location === "undefined") {
    return "ws://127.0.0.1/v1/realtime";
  }
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}/v1/realtime`;
}

function base64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

/**
 * Owns one OpenAI-compatible Realtime transcription socket. It sends only
 * canonical client events and publishes only strictly decoded server events.
 */
export class RealtimeTranscriptionService extends Disposable {
  private readonly stateEmitter = this._register(new Emitter<RealtimeTranscriptionStateEvent>());
  private readonly eventEmitter = this._register(new Emitter<RealtimeTranscriptionEvent>());
  private readonly errorEmitter = this._register(new Emitter<RealtimeTranscriptionError>());
  private socket: RealtimeSocket | null = null;
  private disposed = false;
  private negotiatedHypotheses = false;
  private currentState: RealtimeTranscriptionState = "connecting";
  private currentGeneration: RealtimeSocketGeneration = 0;
  private reconnectDelayMs = RECONNECT_INITIAL_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  /** Fires when connection readiness changes. */
  readonly onState: ServiceEvent<RealtimeTranscriptionStateEvent> = this.stateEmitter.event;
  /** Fires each server event after strict decoding succeeds. */
  readonly onEvent: ServiceEvent<RealtimeTranscriptionEvent> = this.eventEmitter.event;
  /** Fires a recoverable transport or server-event decoding failure. */
  readonly onError: ServiceEvent<RealtimeTranscriptionError> = this.errorEmitter.event;

  constructor(private readonly options: RealtimeTranscriptionOptions = {}) {
    super();
    this.connect();
  }

  /** Current connection readiness. */
  get state(): RealtimeTranscriptionState {
    return this.currentState;
  }

  /** Generation of the current socket or most recent connection attempt. */
  get generation(): RealtimeSocketGeneration {
    return this.currentGeneration;
  }

  /** Opens a fresh relay connection after a recoverable outage. */
  connect(): void {
    if (this.disposed || this.socket !== null) {
      return;
    }
    const generation = this.currentGeneration + 1;
    this.setState("connecting", generation);
    const socketFactory = this.options.socket ?? defaultSocket;
    let socket: RealtimeSocket;
    try {
      socket = socketFactory(socketUrl());
    } catch {
      this.setState("unavailable", generation);
      this.reportError("connection_failed", "connection", null, generation);
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    const onMessage = (event: MessageEvent<unknown>): void => {
      if (this.socket === socket) {
        this.handleMessage(event.data, generation);
      }
    };
    const onError = (): void => {
      if (this.socket === socket) {
        this.socket = null;
        this.resetConnectionState();
        this.setState("unavailable", generation);
        this.reportError("connection_failed", "connection", null, generation);
        socket.close();
        this.scheduleReconnect();
      }
    };
    const onClose = (): void => {
      if (this.socket !== socket) {
        return;
      }
      this.socket = null;
      this.resetConnectionState();
      if (!this.disposed) {
        this.setState("unavailable", generation);
        this.reportError("connection_closed", "connection", null, generation);
        this.scheduleReconnect();
      }
    };
    if (socket.addEventListener !== undefined) {
      socket.addEventListener("message", (event) => onMessage(event as MessageEvent<unknown>));
      socket.addEventListener("error", onError);
      socket.addEventListener("close", onClose);
    } else {
      socket.onmessage = onMessage;
      socket.onerror = onError;
      socket.onclose = onClose;
    }
  }

  /** Appends one exact 24 kHz mono PCM16 block and returns its client event ID. */
  append(audio: ArrayBuffer): RealtimeTranscriptionResult {
    return this.sendClientEvent({
      type: "input_audio_buffer.append",
      audio: base64(audio),
    });
  }

  /** Commits the current input buffer and returns its client event ID. */
  commit(): RealtimeTranscriptionResult {
    return this.sendClientEvent({ type: "input_audio_buffer.commit" });
  }

  /** Clears the current input buffer and returns its client event ID. */
  clear(): RealtimeTranscriptionResult {
    return this.sendClientEvent({ type: "input_audio_buffer.clear" });
  }

  override dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    super.dispose();
  }

  private handleMessage(data: unknown, generation: RealtimeSocketGeneration): void {
    if (typeof data !== "string") {
      this.reportError("invalid_server_event", "session", null, generation);
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      this.reportError("invalid_server_event", "session", null, generation);
      return;
    }
    const event = decodeRealtimeEvent(parsed);
    if (event === null) {
      this.reportError("invalid_server_event", "session", null, generation);
      return;
    }
    if (
      event.type === "conversation.item.input_audio_transcription.delta" &&
      this.negotiatedHypotheses
    ) {
      return;
    }
    this.eventEmitter.fire({ event, generation });

    switch (event.type) {
      case "session.created":
        this.send({
          type: "session.update",
          session: {
            type: "transcription",
            audio: {
              input: {
                format: { type: "audio/pcm", rate: 24_000 },
                noise_reduction: null,
                transcription: {
                  model: "realtime-transcribe",
                  prompt: this.options.prompt ?? "",
                },
                turn_detection: null,
              },
            },
            include: [HYPOTHESIS_INCLUDE],
          },
          event_id: (this.options.eventId ?? defaultEventId)(),
        });
        return;
      case "session.updated":
        this.negotiatedHypotheses =
          event.session.include.length === 1 &&
          event.session.include[0] === HYPOTHESIS_INCLUDE;
        this.reconnectDelayMs = RECONNECT_INITIAL_MS;
        if (this.reconnectTimer !== null) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }
        this.setState("ready", generation);
        return;
      case "input_audio_buffer.committed":
      case "input_audio_buffer.cleared":
      case "conversation.item.created":
      case "conversation.item.input_audio_transcription.hypothesis":
      case "conversation.item.input_audio_transcription.delta":
      case "conversation.item.input_audio_transcription.completed":
      case "conversation.item.input_audio_transcription.failed":
        return;
      case "error":
        return;
      default: {
        const exhaustive: never = event;
        return exhaustive;
      }
    }
  }

  private sendClientEvent(event: Record<string, unknown>): RealtimeTranscriptionResult {
    const eventId = (this.options.eventId ?? defaultEventId)();
    return {
      generation: this.currentGeneration,
      eventId: this.send({ ...event, event_id: eventId }) ? eventId : null,
    };
  }

  private send(event: Record<string, unknown>): boolean {
    const socket = this.socket;
    if (socket === null || socket.readyState !== 1) {
      this.reportError("connection_unavailable");
      return false;
    }
    try {
      socket.send(JSON.stringify(event));
      return true;
    } catch {
      this.reportError("connection_failed");
      return false;
    }
  }

  private setState(
    state: RealtimeTranscriptionState,
    generation: RealtimeSocketGeneration,
  ): void {
    if (this.currentState === state && this.currentGeneration === generation) {
      return;
    }
    this.currentState = state;
    this.currentGeneration = generation;
    this.stateEmitter.fire({ state, generation });
  }

  private resetConnectionState(): void {
    this.negotiatedHypotheses = false;
  }

  private scheduleReconnect(): void {
    if (this.disposed || this.socket !== null || this.reconnectTimer !== null) {
      return;
    }
    const delay = this.reconnectDelayMs;
    this.reconnectDelayMs = Math.min(delay * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }

  private reportError(
    code: string,
    scope: RealtimeTranscriptionError["scope"] = "connection",
    eventId: string | null = null,
    generation: RealtimeSocketGeneration = this.currentGeneration,
  ): void {
    this.errorEmitter.fire({
      code,
      scope,
      eventId,
      recoverable: true,
      generation,
    });
  }
}
