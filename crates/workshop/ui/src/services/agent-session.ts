// The agent-session view model: DOM-free state over one /agents/ws
// socket. The service subscribes to the socket's frame events and keeps
// what the views render: the discovered agent list, the acknowledged
// session, the transcript, and the pending input wait. The transcript is
// derived from the durable event stream plus the ephemeral deltas:
// deltas coalesce into pending items keyed by the reply id stamped on
// every chunk, and the durable event stamped with that id replaces them (the
// ACP chunk-vs-upsert rule the wire layer documents). Views subscribe to
// the change events and read the snapshots; nothing here touches the
// DOM.
//
// Beyond the items, the service keeps the turn's liveness: `generating`
// (the agent is working, so the composer offers stop and the transcript
// shows a tail status), `reconnecting` (the socket dropped and the
// reattach has not been acknowledged), and the reasoning clock that
// times each thought. The clock is injectable so tests stamp exact
// durations.

import { Emitter, type Event } from "@workshop/platform/event";
import { Disposable } from "@workshop/platform/lifecycle";
import type {
  AgentDeltaFrame,
  AgentEventFrame,
  AgentSessionFrame,
} from "./protocol";

/**
 * The slice of the agent socket this service consumes; `AgentSocket`
 * satisfies it structurally, and tests hand in a scripted fake. The
 * service never owns the wire: connect, reconnect, and disposal belong
 * to the caller that constructed the socket.
 */
export interface AgentSessionWire {
  readonly onAgents: Event<string[]>;
  readonly onSession: Event<AgentSessionFrame>;
  readonly onEvent: Event<AgentEventFrame>;
  readonly onDelta: Event<AgentDeltaFrame>;
  readonly onInputRequired: Event<string>;
  readonly onInputCancelled: Event<string>;
  readonly onError: Event<string>;
  /** Fires when the socket drops; the socket reconnects on its own. */
  readonly onDisconnect: Event<void>;
  launch(agent: string): boolean;
  respond(token: string, text: string): boolean;
  /** Fires the turn-cancel; false when the socket is down and nothing was sent. */
  cancelTurn(): boolean;
}

/** One call of a tool-call batch: its id, name, rendered arguments, and bound tool. */
export interface ToolCallRow {
  readonly id: string;
  readonly name: string;
  /** The call arguments as compact JSON, or "" when the call had none. */
  readonly args: string;
  /**
   * The bound tool id the call resolved to, as the wire's one
   * slash-separated string (`web/search`), or null for a Lua-local tool,
   * a task built-in, or an entry the wire did not stamp. Views render a
   * null tool as a generic tool.
   */
  readonly tool: string | null;
}

/** Text the operator sent, as the durable `user_message` event recorded it. */
export interface UserItem {
  readonly kind: "user";
  readonly text: string;
}

/**
 * An assistant reply: pending while it is coalesced deltas, settled once
 * the durable `agent_message` event replaces them.
 */
export interface ReplyItem {
  readonly kind: "reply";
  readonly text: string;
  /** The producing model, when the event named the attribution. */
  readonly model: string | null;
  /** True while the item is coalesced deltas awaiting the durable event. */
  readonly pending: boolean;
  /** The reply id coalescing this item's deltas away, if one is known. */
  readonly reply: number | null;
}

/**
 * A model reasoning block, streamed and settled exactly as a reply is.
 * `startedAt` and `endedAt` are readings of the service's clock: the
 * round's first reasoning delta, and the moment the thinking stopped.
 * Both are null for a thought the service never watched stream (a replay
 * after a reattach), whose duration is therefore unknown.
 */
export interface ReasoningItem {
  readonly kind: "reasoning";
  readonly text: string;
  readonly model: string | null;
  readonly pending: boolean;
  readonly reply: number | null;
  readonly startedAt: number | null;
  readonly endedAt: number | null;
}

/** A batch of tool calls the model requested, one row per call. */
export interface ToolCallItem {
  readonly kind: "tool-call";
  readonly calls: readonly ToolCallRow[];
  /** The raw batch content: the fallback when the JSON did not parse. */
  readonly text: string;
  readonly model: string | null;
}

/** The result of one dispatched tool call. */
export interface ToolResultItem {
  readonly kind: "tool-result";
  /** The provider-issued call id, scoped by turn (providers recycle ids). */
  readonly toolCallId: string | null;
  readonly text: string;
}

/** A server error frame, or a local send failure the server cannot report. */
export interface ErrorItem {
  readonly kind: "error";
  readonly message: string;
  /**
   * The popup title a local failure carries ("Connection failed"), or
   * null for a server error, which the popup titles "Connection Error".
   */
  readonly title: string | null;
}

/** One renderable entry of the session transcript, in display order. */
export type TranscriptItem =
  | UserItem
  | ReplyItem
  | ReasoningItem
  | ToolCallItem
  | ToolResultItem
  | ErrorItem;

/** The two delta channels a round streams, as transcript item kinds. */
type StreamKind = "reply" | "reasoning";

/** The title and message of the local failure a send on a down socket folds. */
const CONNECTION_FAILED_TITLE = "Connection failed";
const CONNECTION_FAILED_MESSAGE =
  "The connection was interrupted. Please check your network connection and try again.";

/**
 * Parses a `tool_call` event's content - the JSON array of the batch's
 * calls - into display rows. Anything but a well-formed array degrades to
 * an empty row list so the view falls back to the raw text: the content
 * is model-era data crossing a trust boundary, and a batch the server
 * failed to serialize must still render rather than vanish.
 */
function parseToolCalls(content: string): readonly ToolCallRow[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) {
    return [];
  }
  const rows: ToolCallRow[] = [];
  for (const entry of parsed) {
    if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
      continue;
    }
    const record = entry as Record<string, unknown>;
    rows.push({
      id: typeof record.id === "string" ? record.id : "",
      name: typeof record.name === "string" ? record.name : "",
      args: "arguments" in record ? JSON.stringify(record.arguments) : "",
      // A tool id is never empty, so an empty string is as malformed as a
      // number: both read as the generic tool.
      tool: typeof record.tool === "string" && record.tool !== "" ? record.tool : null,
    });
  }
  return rows;
}

/**
 * The state behind one agent session surface. Reads arrive as socket
 * events; the service folds them into snapshots and fires the matching
 * change event after every fold, so a view repaints from `agents`,
 * `session`, `items`, `pendingInputToken`, `generating`, and
 * `reconnecting` alone.
 */
export class AgentSessionService extends Disposable {
  private agentList: readonly string[] = [];
  private acknowledged: AgentSessionFrame | null = null;
  private transcript: TranscriptItem[] = [];
  /**
   * The highest reply id a durable `agent_message` or `tool_call` event
   * has settled. Deltas at or below it are late chunks from the cancel
   * grace window; folding them in would open a pending item nothing will
   * ever supersede.
   */
  private settled = -1;
  /**
   * True from a sent cancel until the turn's work restarts (an answer, an
   * input request, or a session acknowledgment). A reasoning chunk that
   * arrives in that window is a late straggler from the cancel grace
   * period; it opens already ended, so the thought never reads as
   * streaming under an idle composer.
   */
  private stopped = false;
  private pinnedToken: string | null = null;
  private working = false;
  private dropped = false;

  private readonly _onDidChangeAgents = this._register(new Emitter<readonly string[]>());
  /** Fires with every pushed agent list - a complete snapshot per connect. */
  readonly onDidChangeAgents: Event<readonly string[]> = this._onDidChangeAgents.event;

  private readonly _onDidChangeSession = this._register(new Emitter<AgentSessionFrame>());
  /** Fires on every session acknowledgment, launch and reattach alike. */
  readonly onDidChangeSession: Event<AgentSessionFrame> = this._onDidChangeSession.event;

  private readonly _onDidChangeTranscript = this._register(new Emitter<void>());
  /** Fires after every transcript fold; read `items` for the snapshot. */
  readonly onDidChangeTranscript: Event<void> = this._onDidChangeTranscript.event;

  private readonly _onDidChangePendingInput = this._register(new Emitter<string | null>());
  /** Fires with the pinned wait token, or null when no wait is open. */
  readonly onDidChangePendingInput: Event<string | null> = this._onDidChangePendingInput.event;

  private readonly _onDidChangeGenerating = this._register(new Emitter<boolean>());
  /** Fires with the new value whenever `generating` really changes. */
  readonly onDidChangeGenerating: Event<boolean> = this._onDidChangeGenerating.event;

  private readonly _onDidChangeReconnecting = this._register(new Emitter<boolean>());
  /** Fires with the new value whenever `reconnecting` really changes. */
  readonly onDidChangeReconnecting: Event<boolean> = this._onDidChangeReconnecting.event;

  private readonly _onError = this._register(new Emitter<string>());
  /** Fires for every error folded into the transcript, message as shown. */
  readonly onError: Event<string> = this._onError.event;

  /**
   * `now` is the clock the reasoning stamps read; it defaults to
   * `performance.now()`, and tests inject their own so durations are exact.
   */
  constructor(
    private readonly wire: AgentSessionWire,
    private readonly now: () => number = () => performance.now(),
  ) {
    super();
    this._register(
      wire.onAgents((agents) => {
        this.agentList = agents;
        this._onDidChangeAgents.fire(this.agentList);
      }),
    );
    this._register(wire.onSession((frame) => this.acknowledge(frame)));
    this._register(wire.onEvent((frame) => this.foldEvent(frame)));
    this._register(wire.onDelta((frame) => this.foldDelta(frame)));
    this._register(
      wire.onInputRequired((token) => {
        // The agent is waiting on the operator: no thinking is still
        // running, and nothing is generating until the answer lands.
        const closed = this.closeThinking(null);
        // The relaunched agent is waiting: the cancel grace window is over.
        this.stopped = false;
        this.setGenerating(false);
        this.setPinnedToken(token);
        if (closed) {
          this._onDidChangeTranscript.fire();
        }
      }),
    );
    this._register(
      wire.onInputCancelled((token) => {
        // Only the announced wait dies; a newer pin stays.
        if (this.pinnedToken === token) {
          this.setPinnedToken(null);
        }
      }),
    );
    this._register(wire.onError((message) => this.foldError(message, null)));
    this._register(wire.onDisconnect(() => this.setReconnecting(true)));
  }

  /** The discovered agent names, as last pushed by the server. */
  get agents(): readonly string[] {
    return this.agentList;
  }

  /** The acknowledged session, or null before a launch is answered. */
  get session(): AgentSessionFrame | null {
    return this.acknowledged;
  }

  /** The transcript, in display order. */
  get items(): readonly TranscriptItem[] {
    return this.transcript;
  }

  /** The pending wait's token, or null while the agent is working. */
  get pendingInputToken(): string | null {
    return this.pinnedToken;
  }

  /**
   * True while the agent is working on the operator's last message: on
   * after a send (and after an answered tool call), off when the reply
   * lands, a wait opens, an error arrives, or a cancel is sent.
   */
  get generating(): boolean {
    return this.working;
  }

  /**
   * True from a socket drop until the reattach is acknowledged; the
   * transcript can't advance meanwhile.
   */
  get reconnecting(): boolean {
    return this.dropped;
  }

  /**
   * Asks the server to launch the named agent; the acknowledgment (or an
   * error frame for a refused launch) arrives on the wire. False when
   * the socket is down and nothing was sent.
   */
  launch(agent: string): boolean {
    return this.wire.launch(agent);
  }

  /**
   * Answers the pending wait with the operator's text, byte-exact. The
   * text never enters the transcript here: the server records it and the
   * durable `user_message` event renders it, so the view shows exactly
   * what the log holds. False when no wait is pinned or the socket is
   * down; a failed send keeps the pin (the wait is still open
   * server-side) and folds a local error item, because a downed socket
   * is the one failure the server can never report. A sent answer starts
   * the turn's work, so `generating` turns on.
   */
  respond(text: string): boolean {
    const token = this.pinnedToken;
    if (token === null) {
      return false;
    }
    if (!this.wire.respond(token, text)) {
      this.foldError(CONNECTION_FAILED_MESSAGE, CONNECTION_FAILED_TITLE);
      return false;
    }
    // The answer restarts the turn's work, so a stop no longer applies.
    this.stopped = false;
    // The token is single-use; the response just spent it.
    this.setPinnedToken(null);
    this.setGenerating(true);
    return true;
  }

  /**
   * Cancels the running turn. The server answers a cancel with nothing -
   * cancellation is a stop reason, not an error - so after a successful
   * send the service settles its own view: `generating` turns off and any
   * open thinking ends. False when the socket is down, with nothing
   * changed.
   */
  cancelTurn(): boolean {
    if (!this.wire.cancelTurn()) {
      return false;
    }
    this.stopped = true;
    const closed = this.closeThinking(null);
    this.setGenerating(false);
    if (closed) {
      this._onDidChangeTranscript.fire();
    }
    return true;
  }

  /**
   * Folds a session acknowledgment. A different session id means a
   * different event log replaying from index zero, so the transcript
   * resets with it; a same-session reattach keeps the transcript (the
   * socket's cursor already deduplicates the replay). Wait pinning
   * resets on every acknowledgment: the server resends unresolved waits
   * right after, so a stale prompt vanishes by its token's absence. The
   * acknowledgment also ends a reconnect.
   */
  private acknowledge(frame: AgentSessionFrame): void {
    const changedSession =
      this.acknowledged === null || this.acknowledged.session !== frame.session;
    this.acknowledged = frame;
    this.stopped = false;
    if (changedSession) {
      this.transcript = [];
      this.settled = -1;
      this.setGenerating(false);
      this._onDidChangeTranscript.fire();
    }
    this.setPinnedToken(null);
    this.setReconnecting(false);
    this._onDidChangeSession.fire(frame);
  }

  /** Folds one durable event into the transcript. */
  private foldEvent(frame: AgentEventFrame): void {
    const event = frame.event;
    const reply = frame.reply ?? null;
    switch (event.kind) {
      case "user_message": {
        this.transcript.push({ kind: "user", text: event.content });
        break;
      }
      case "agent_message": {
        this.settle(reply);
        this.transcript.push({
          kind: "reply",
          text: event.content,
          model: event.model ?? null,
          pending: false,
          reply,
        });
        // A reply always ends the turn: the chat agent loops ask, then
        // model, so nothing runs after one.
        this.setGenerating(false);
        break;
      }
      case "agent_thought": {
        // A thought settles only its own channel: the round stays open
        // and its text deltas keep streaming toward the reply. The
        // durable item inherits the pending item's clock; a round that
        // never streamed text (a tool-call round) has no text delta to
        // end its thinking, so the durable thought ends it.
        const pending = this.pendingReasoning(reply);
        const startedAt = pending?.startedAt ?? null;
        const endedAt = pending?.endedAt ?? (startedAt === null ? null : this.now());
        this.dropPending(reply, "reasoning");
        this.transcript.push({
          kind: "reasoning",
          text: event.content,
          model: event.model ?? null,
          pending: false,
          reply,
          startedAt,
          endedAt,
        });
        break;
      }
      case "tool_call": {
        this.settle(reply);
        this.transcript.push({
          kind: "tool-call",
          calls: parseToolCalls(event.content),
          text: event.content,
          model: event.model ?? null,
        });
        break;
      }
      case "tool_call_update": {
        // An answered model call means the agent has its result and is
        // working again - this is also what ends the wait of a model's
        // own ask. Checked before the result lands so the scan reads
        // only the calls.
        const toolCallId = event.tool_call_id ?? null;
        if (toolCallId !== null && this.answersModelCall(toolCallId)) {
          this.setGenerating(true);
        }
        this.transcript.push({
          kind: "tool-result",
          toolCallId,
          text: event.content,
        });
        break;
      }
      default: {
        // The Rust kind enum is non-exhaustive: future kinds arrive as
        // labels outside the union and render nothing rather than
        // breaking the feed.
        return;
      }
    }
    this._onDidChangeTranscript.fire();
  }

  /**
   * Folds one ephemeral chunk: appended to its round's pending item on
   * the matching channel, or opening that item when the chunk is the
   * round's first. A reasoning chunk that opens its item starts the
   * round's clock; a text chunk ends the same round's thinking.
   */
  private foldDelta(frame: AgentDeltaFrame): void {
    if (frame.reply <= this.settled) {
      // A late chunk whose durable superseder already rendered.
      return;
    }
    const kind: StreamKind = frame.kind === "reasoning" ? "reasoning" : "reply";
    if (kind === "reply") {
      this.closeThinking(frame.reply);
    }
    const index = this.findPending(frame.reply, kind);
    if (index === -1) {
      this.transcript.push(
        kind === "reasoning"
          ? {
              kind: "reasoning",
              text: frame.content,
              model: null,
              pending: true,
              reply: frame.reply,
              // After Stop the chunk is a late straggler: it opens ended,
              // with no start, so it reads as a finished thought.
              startedAt: this.stopped ? null : this.now(),
              endedAt: this.stopped ? this.now() : null,
            }
          : { kind: "reply", text: frame.content, model: null, pending: true, reply: frame.reply },
      );
    } else {
      const existing = this.transcript[index];
      if (existing !== undefined && (existing.kind === "reply" || existing.kind === "reasoning")) {
        // Replaced, not mutated: the view diffs items by identity.
        this.transcript[index] = { ...existing, text: existing.text + frame.content };
      }
    }
    this._onDidChangeTranscript.fire();
  }

  /**
   * Folds an error into the transcript and announces it. An error ends
   * the turn, so open thinking ends and `generating` turns off.
   */
  private foldError(message: string, title: string | null): void {
    this.closeThinking(null);
    this.setGenerating(false);
    this.transcript.push({ kind: "error", message, title });
    this._onDidChangeTranscript.fire();
    this._onError.fire(message);
  }

  /**
   * Marks a round settled by its durable reply or tool-call event: both
   * channels' pending items die, superseded by the event that follows.
   */
  private settle(reply: number | null): void {
    if (reply === null) {
      return;
    }
    this.settled = Math.max(this.settled, reply);
    this.dropPending(reply, "reply");
    this.dropPending(reply, "reasoning");
  }

  /** Removes the pending item for one round's channel, if it is open. */
  private dropPending(reply: number | null, kind: StreamKind): void {
    if (reply === null) {
      return;
    }
    const index = this.findPending(reply, kind);
    if (index !== -1) {
      this.transcript.splice(index, 1);
    }
  }

  /** The pending reasoning item of one round, or undefined. */
  private pendingReasoning(reply: number | null): ReasoningItem | undefined {
    if (reply === null) {
      return undefined;
    }
    const item = this.transcript[this.findPending(reply, "reasoning")];
    return item?.kind === "reasoning" ? item : undefined;
  }

  /**
   * Ends every open thinking - the pending reasoning items that have no
   * end yet - for one round, or for every round when `reply` is null.
   * Items are replaced, not mutated. Returns whether anything changed;
   * the caller owns announcing it.
   */
  private closeThinking(reply: number | null): boolean {
    let changed = false;
    let at: number | null = null;
    for (let index = 0; index < this.transcript.length; index++) {
      const item = this.transcript[index];
      if (
        item !== undefined &&
        item.kind === "reasoning" &&
        item.pending &&
        item.endedAt === null &&
        (reply === null || item.reply === reply)
      ) {
        at ??= this.now();
        this.transcript[index] = { ...item, endedAt: at };
        changed = true;
      }
    }
    return changed;
  }

  /**
   * True when `toolCallId` names a call of a model tool-call batch in the
   * current turn: the scan runs back from the tail and stops at the
   * turn's user item, because providers recycle ids like `call_1` across
   * turns.
   */
  private answersModelCall(toolCallId: string): boolean {
    for (let index = this.transcript.length - 1; index >= 0; index--) {
      const item = this.transcript[index];
      if (item === undefined) {
        continue;
      }
      if (item.kind === "user") {
        return false;
      }
      if (item.kind === "tool-call" && item.calls.some((call) => call.id === toolCallId)) {
        return true;
      }
    }
    return false;
  }

  /**
   * The index of the pending item for one round's channel, or -1. Scans
   * from the tail: pending items always sit near it, because rounds are
   * sequential.
   */
  private findPending(reply: number, kind: StreamKind): number {
    for (let index = this.transcript.length - 1; index >= 0; index--) {
      const item = this.transcript[index];
      if (item !== undefined && item.kind === kind && item.pending && item.reply === reply) {
        return index;
      }
    }
    return -1;
  }

  /** Pins or clears the wait token, firing only on a real change. */
  private setPinnedToken(token: string | null): void {
    if (this.pinnedToken === token) {
      return;
    }
    this.pinnedToken = token;
    this._onDidChangePendingInput.fire(token);
  }

  /** Sets `generating`, firing only on a real change. */
  private setGenerating(value: boolean): void {
    if (this.working === value) {
      return;
    }
    this.working = value;
    this._onDidChangeGenerating.fire(value);
  }

  /** Sets `reconnecting`, firing only on a real change. */
  private setReconnecting(value: boolean): void {
    if (this.dropped === value) {
      return;
    }
    this.dropped = value;
    this._onDidChangeReconnecting.fire(value);
  }
}
