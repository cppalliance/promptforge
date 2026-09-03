// The agent-session socket: one WebSocket to /agents/ws serving one agent
// session - agent windows are modal, so the socket's whole life is one
// session plus the agent list that precedes it. The frame shapes live in
// protocol.ts; the Rust half of the routing is
// crates/workshop-server/src/session_agents/socket.rs.
//
// Routing follows the SPA's delivery discipline, one class per frame:
//
// - The `agents` list is an ephemeral complete snapshot, resent by the
//   server on every connect: the newest push supersedes every older one.
// - `agent_session` is the durable direct reply to a launch or attach; the
//   acknowledged session id is retained here so a reconnect reattaches.
// - `agent_event` frames are durable: the server drains them in log order,
//   and an attach replays the log from index zero, so this socket keeps a
//   per-session cursor and drops already-delivered indices - the client
//   half of the durable cursor-and-replay promise, which is also how a
//   reattach's replay stays duplicate-free for consumers.
// - `agent_delta` frames are ephemeral: they may drop under lag and are
//   never buffered here; each carries the `reply` id of the durable event
//   that will supersede it, so the renderer coalesces chunks by that id
//   and the completed-reply event is the repair path.
// - `input_required` / `input_cancelled` are durable through the server's
//   wait registry: unresolved waits are resent on every attach, so a
//   consumer re-pins from the resent set after each session acknowledgment
//   and a stale prompt vanishes by its token's absence.
//
// No boot queue rides here: the WorkshopSocket queue guards the app-boot
// race where the server's first pushes beat handler wiring, but this
// socket is constructed and subscribed by its owning view before
// `connect()` is called, so no push can precede its handlers.

import { Emitter, type Event } from "../base/event";
import { Disposable, toDisposable } from "../base/lifecycle";
import type {
  AgentCancelFrame,
  AgentDeltaFrame,
  AgentEventFrame,
  AgentSessionFrame,
  AttachFrame,
  InputResponseFrame,
  LaunchFrame,
} from "./protocol";

function defaultUrl(): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/agents/ws`;
}

// Reconnect backoff, matching the workshop socket's: the first retry waits
// a second, each failure doubles it, and the cap keeps a down server from
// pushing the wait past 30 s.
const RECONNECT_INITIAL_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

/**
 * The loosely-typed inbound frame: exactly the fields routing reads,
 * narrowed per `type` before delivery. The full payloads ride through as
 * their protocol.ts types once the envelope checks pass.
 */
interface AgentServerFrame {
  type?: unknown;
  agents?: unknown;
  session?: unknown;
  agent?: unknown;
  index?: unknown;
  content?: unknown;
  token?: unknown;
  message?: unknown;
}

/**
 * The client of one /agents/ws socket. Sessions outlive sockets: after a
 * dropout the socket reconnects with backoff and, when a session was
 * acknowledged, reattaches to it - the server replays the event log from
 * index zero (deduplicated here by the event cursor) and re-announces
 * unresolved waits. A reattach refused because the session ended while
 * disconnected surfaces as an `onError` message, exactly as the server
 * sent it.
 */
export class AgentSocket extends Disposable {
  private socket: WebSocket | null = null;
  private reconnectDelayMs = RECONNECT_INITIAL_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  /** The acknowledged session, retained so a reconnect reattaches. */
  private acknowledged: AgentSessionFrame | null = null;
  /** The next event-log index to deliver; everything below it already was. */
  private nextIndex = 0;

  private readonly _onAgents = this._register(new Emitter<string[]>());
  /** Fires for every pushed agent list - a complete snapshot per connect. */
  readonly onAgents: Event<string[]> = this._onAgents.event;

  private readonly _onSession = this._register(new Emitter<AgentSessionFrame>());
  /**
   * Fires for every session acknowledgment, including the one a reconnect's
   * reattach earns. Consumers reset wait pinning here and re-pin from the
   * `input_required` frames the server resends after it.
   */
  readonly onSession: Event<AgentSessionFrame> = this._onSession.event;

  private readonly _onEvent = this._register(new Emitter<AgentEventFrame>());
  /** Fires once per event-log entry, in log order, replay-deduplicated. */
  readonly onEvent: Event<AgentEventFrame> = this._onEvent.event;

  private readonly _onDelta = this._register(new Emitter<AgentDeltaFrame>());
  /**
   * Fires for every live streaming chunk. Ephemeral: chunks may drop under
   * lag; coalesce them by their `reply` id and replace them with the
   * superseding durable event when it arrives on `onEvent`.
   */
  readonly onDelta: Event<AgentDeltaFrame> = this._onDelta.event;

  private readonly _onInputRequired = this._register(new Emitter<string>());
  /** Fires with the wait token an `input_response` must echo. */
  readonly onInputRequired: Event<string> = this._onInputRequired.event;

  private readonly _onInputCancelled = this._register(new Emitter<string>());
  /** Fires with the token of a wait that died unresolved. */
  readonly onInputCancelled: Event<string> = this._onInputCancelled.event;

  private readonly _onError = this._register(new Emitter<string>());
  /** Fires for every server error frame, message as sent. */
  readonly onError: Event<string> = this._onError.event;

  private readonly _onDisconnect = this._register(new Emitter<void>());
  /** Fires when the socket disconnects; a reconnect is already scheduled. */
  readonly onDisconnect: Event<void> = this._onDisconnect.event;

  constructor(private readonly url: string = defaultUrl()) {
    super();
    // Disposal silences the socket before closing it (onclose detached
    // first), so teardown is never mistaken for a dropout: no disconnect
    // fan-out, no reconnect backoff.
    this._register(
      toDisposable(() => {
        if (this.reconnectTimer !== null) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }
        const socket = this.socket;
        if (socket) {
          socket.onclose = null;
          socket.close();
          this.socket = null;
        }
      }),
    );
  }

  /** Opens the socket unless it is already open or opening. */
  connect(): void {
    if (this.socket) {
      return;
    }
    const socket = new WebSocket(this.url);
    this.socket = socket;
    socket.onopen = () => {
      if (this.reconnectTimer !== null) {
        clearTimeout(this.reconnectTimer);
        this.reconnectTimer = null;
      }
      this.reconnectDelayMs = RECONNECT_INITIAL_MS;
      // Sessions outlive sockets: a fresh connection reattaches to the
      // acknowledged session. The replay from index zero that follows is
      // deduplicated by the event cursor.
      if (this.acknowledged) {
        this.sendFrame({
          type: "attach",
          session: this.acknowledged.session,
        } satisfies AttachFrame);
      }
    };
    socket.onerror = () => {
      if (this.socket === socket) this.socket = null;
    };
    socket.onmessage = (event: MessageEvent) => this.route(event);
    socket.onclose = () => {
      if (this.socket === socket) this.socket = null;
      this._onDisconnect.fire(undefined);
      this.scheduleReconnect();
    };
  }

  /**
   * Sends one `launch` frame starting a session of the named agent. False
   * when the socket is down, nothing sent; the server answers with an
   * `agent_session` acknowledgment, or an error frame for an unknown name.
   */
  launch(agent: string): boolean {
    return this.sendFrame({ type: "launch", agent } satisfies LaunchFrame);
  }

  /**
   * Sends one `attach` frame joining the running session with this id.
   * The failure contract matches `launch`: false when the socket is down.
   */
  attach(session: string): boolean {
    return this.sendFrame({ type: "attach", session } satisfies AttachFrame);
  }

  /**
   * Answers an `input_required` prompt: the operator's text, byte-exact,
   * echoing the wait's token. False when the socket is down.
   */
  respond(token: string, text: string): boolean {
    return this.sendFrame({ type: "input_response", token, text } satisfies InputResponseFrame);
  }

  /**
   * Fires the session's turn-cancel. The server answers with nothing -
   * cancellation is a stop reason, not an error - so the caller settles
   * its own view; pending waits announce their deaths as
   * `input_cancelled`. False when the socket is down.
   */
  cancelTurn(): boolean {
    return this.sendFrame({ type: "cancel" } satisfies AgentCancelFrame);
  }

  /** Sends one JSON frame; false when the socket is down or the send threw. */
  private sendFrame(frame: Record<string, unknown>): boolean {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return false;
    }
    try {
      socket.send(JSON.stringify(frame));
      return true;
    } catch {
      // A send that throws mid-close is the same failure as a closed
      // socket; the close handler carries the cleanup.
      return false;
    }
  }

  /**
   * Schedules the next reconnect attempt with exponential backoff. One
   * timer at a time: a close while an attempt is already waiting does not
   * stack a second.
   */
  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) {
      return;
    }
    const delay = this.reconnectDelayMs;
    this.reconnectDelayMs = Math.min(delay * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }

  private route(event: MessageEvent): void {
    let frame: AgentServerFrame;
    try {
      frame = JSON.parse(String(event.data)) as AgentServerFrame;
    } catch {
      // A non-JSON frame carries no agent event; keep reading.
      return;
    }
    if (frame.type === "agents") {
      this._onAgents.fire(Array.isArray(frame.agents) ? (frame.agents as string[]) : []);
      return;
    }
    if (
      frame.type === "agent_session" &&
      typeof frame.session === "string" &&
      typeof frame.agent === "string"
    ) {
      // A different session's log starts over at index zero, so the cursor
      // resets with it - otherwise a launch after a refused reattach (the
      // session died while disconnected) would silently swallow the new
      // log's head. A reattach to the same session keeps the cursor: that
      // dedup is the replay contract.
      if (this.acknowledged !== null && this.acknowledged.session !== frame.session) {
        this.nextIndex = 0;
      }
      this.acknowledged = frame as unknown as AgentSessionFrame;
      this._onSession.fire(this.acknowledged);
      return;
    }
    if (frame.type === "agent_event" && typeof frame.index === "number") {
      // The durable cursor: an attach replays the log from index zero, so
      // everything below the cursor was already delivered and drops here.
      if (frame.index < this.nextIndex) {
        return;
      }
      this.nextIndex = frame.index + 1;
      this._onEvent.fire(frame as unknown as AgentEventFrame);
      return;
    }
    if (frame.type === "agent_delta" && typeof frame.content === "string") {
      this._onDelta.fire(frame as unknown as AgentDeltaFrame);
      return;
    }
    if (frame.type === "input_required" && typeof frame.token === "string") {
      this._onInputRequired.fire(frame.token);
      return;
    }
    if (frame.type === "input_cancelled" && typeof frame.token === "string") {
      this._onInputCancelled.fire(frame.token);
      return;
    }
    if (frame.type === "error") {
      this._onError.fire(
        typeof frame.message === "string" && frame.message !== ""
          ? frame.message
          : "the agent session failed",
      );
    }
  }
}
