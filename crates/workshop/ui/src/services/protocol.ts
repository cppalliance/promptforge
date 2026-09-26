// The pure wire types of the workshop protocol: the JSON frame and payload
// shapes exchanged with the server over /ws, /agents/ws, and /v1/models,
// plus the guards that narrow parsed inbound frames to them. The socket
// logic that sends and routes these frames stays in workshop-socket.ts and
// agent-socket.ts. The
// Rust half of this contract is
// crates/workshop/protocol/src plus, for the agent-session frame family,
// crates/workshop/server/src/agents/wire.rs; the files
// cross-cite each other so a shape change touches both or neither. The
// agent-session frame family is additionally pinned by the shared fixture
// crates/workshop/server/tests/fixtures/agent-frames.json,
// asserted as the same JSON by both suites (test/agent-wire-fixtures.mjs
// here, the workshop-server wire tests there), so drift on either side fails
// that side's tests. The workshop-socket frame family is pinned the same
// way by crates/workshop/protocol/tests/fixtures/workshop-frames.json,
// asserted by test/workshop-wire-fixtures.mjs here and the workshop_frames
// fixture test there.

/** One status bar update, as sent by the server. */
export interface StatusFrame {
  type: "status";
  label: string;
  description: string;
  severity: "info" | "debug" | "error";
  activity: "general" | "thinking" | "generating";
  /**
   * Whether work is in flight: the status bar shows its barberpole while
   * set. The label says what the work is; there is no fraction on the wire.
   */
  busy: boolean;
}

/** One entry of the gateway's model catalog, as fetched or pushed. */
export interface CatalogModel {
  id: string;
  description?: string;
}

/** A pushed model catalog, sent when the gateway comes back after an outage. */
export interface ModelsFrame {
  type: "models";
  models: CatalogModel[];
}

/**
 * One pushed workbench snapshot: the server-owned Model-menu state.
 * Absent options are `null`, never omitted keys - every push is the
 * complete menu state. The server computes `chat_ready` (catalog
 * non-empty, a model selected, no switch in flight, gateway reachable);
 * the UI never derives it. `switching` names the target when it is a
 * profile; `switch_in_flight` is true for any in-flight target,
 * including no profile.
 */
export interface WorkbenchFrame {
  type: "workbench";
  profiles: string[];
  active: string | null;
  /**
   * The profile a switch is loading, or null both when no switch runs
   * and when the in-flight switch selects no profile.
   */
  switching: string | null;
  /** Whether a switch is in flight, whatever its target. */
  switch_in_flight: boolean;
  selected: string | null;
  chat_ready: boolean;
}

/**
 * The client frame selecting the chat model:
 * `{"type":"select_model","model":"..."}`. The server validates the id
 * against the retained catalog and publishes a fresh workbench snapshot
 * on success; an unknown model is refused with an `error` frame.
 */
export interface SelectModelFrame {
  type: "select_model";
  model: string;
}

/**
 * The client frame selecting a gateway profile:
 * `{"type":"switch_profile","name":"..."}`, with an explicit `null` name
 * selecting no profile. The key is required; an absent `name` is refused.
 */
export interface SwitchProfileFrame {
  type: "switch_profile";
  name: string | null;
}

/**
 * A failure report answered to one inbound frame: `message` plus the
 * request's `id`, echoed verbatim when the request had one. The
 * agent-session socket also pushes id-less error frames for session-level
 * failures.
 */
export interface ErrorFrame {
  type: "error";
  message: string;
  id?: unknown;
}

// --- Agent-session frames (/agents/ws) --------------------------------------
// The Rust half of this family is the session frame structs in
// crates/workshop/server/src/agents/wire.rs, the input-wait frames in
// crates/workshop/protocol/src/input.rs, and the routing in
// crates/workshop/server/src/agents/socket.rs. Delivery classes mirror the Rust docs:
// durable frames deliver exactly (the event log's per-client cursor and the
// wait registry's resend-on-attach are the repair paths), ephemeral frames
// may drop under lag and repair from a complete snapshot or a superseding
// durable event.

/**
 * The kind of one agent event, following the Agent Client Protocol
 * `sessionUpdate` names. Mirrors `AgentEventKind` in workshop-server
 * (src/agents/wire.rs). Future kinds (`plan`, tool-status updates) may
 * arrive as labels outside this union, so renderers matching on kinds
 * tolerate unknown labels through a wildcard arm.
 */
export type AgentEventKind =
  | "agent_message"
  | "tool_call"
  | "tool_call_update"
  | "agent_thought"
  | "user_message";

/** Token accounting for one model call, as the backend reported it. */
export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  cached_tokens?: number;
  reasoning_tokens?: number;
}

/** llama.cpp `timings` for one call, as the server reported them. */
export interface LlamaTimings {
  prompt_n: number;
  prompt_ms: number;
  prompt_per_second: number;
  predicted_n: number;
  predicted_ms: number;
  predicted_per_second: number;
  draft_n: number;
  draft_n_accepted: number;
}

/** vLLM per-request metrics; vLLM omits what it did not measure. */
export interface VllmMetrics {
  time_to_first_token_ms?: number;
  generation_time_ms?: number;
  queue_time_ms?: number;
  mean_itl_ms?: number;
  tokens_per_second?: number;
}

/** Timing one call end to end, measured by the calling client's clock. */
export interface ClientTiming {
  ttft_ms?: number;
  mean_itl_ms?: number;
  e2e_ms: number;
}

/** Everything measured about one model call, from every reporting source. */
export interface CallMetrics {
  usage?: Usage;
  llama?: LlamaTimings;
  vllm?: VllmMetrics;
  client?: ClientTiming;
}

/**
 * One durable record of something that happened during an agent run,
 * mirroring `AgentEvent` in workshop-server (src/agents/wire.rs): the
 * engine's content event projected onto the wire. `content` and every other
 * free-text field is untrusted model-, tool-, or user-authored data. Absent
 * optional fields are omitted keys on the wire, never `null`.
 */
export interface AgentEvent {
  kind: AgentEventKind;
  /** The reporting scope: for agent sessions, the agent's name. */
  section: string;
  turn: number;
  /** The kind-specific untrusted payload. */
  content: string;
  /** The producing model, on model-attributed kinds. */
  model?: string;
  /**
   * The provider-issued tool-call id, on tool kinds. Providers recycle ids
   * like `call_1` across rounds, so consumers scope the id by turn.
   */
  tool_call_id?: string;
  finish_reason?: string;
  metrics?: CallMetrics;
}

/**
 * The agent list pushed when an /agents/ws socket connects. Ephemeral:
 * every push is the complete discovered list, resent on every connect;
 * there is no incremental form to lose.
 */
export interface AgentsFrame {
  type: "agents";
  agents: string[];
}

/**
 * The direct reply to a `launch` or `attach` frame. Durable: a per-request
 * reply sent by the loop that owns the socket. The client keeps the session
 * id to reattach after a disconnect - sessions outlive sockets.
 */
export interface AgentSessionFrame {
  type: "agent_session";
  session: string;
  agent: string;
}

/**
 * One durable entry of an agent session's event log. `index` is the
 * entry's position in the log; attach replays the log from index zero, so
 * a per-client cursor over `index` recovers everything past it and drops
 * duplicates. `reply` is present on the model-round content kinds
 * (`agent_thought`, `agent_message`, `tool_call`): the id that coalesces
 * the round's ephemeral deltas away.
 */
export interface AgentEventFrame {
  type: "agent_event";
  index: number;
  reply?: number;
  event: AgentEvent;
}

/** Which streaming side channel one agent delta belongs to. */
export type AgentDeltaKind = "text" | "reasoning";

/**
 * One live streaming chunk of an agent's model round. Ephemeral: deltas
 * go out on a bounded broadcast and may drop under lag; the completed-reply
 * event is the repair path. Every delta is stamped with the `reply` id of
 * the durable event that will supersede it, so the SPA coalesces chunks by
 * that id and replaces them when the event arrives (the ACP messageId
 * chunk-vs-upsert rule).
 */
export interface AgentDeltaFrame {
  type: "agent_delta";
  kind: AgentDeltaKind;
  content: string;
  reply: number;
}

/**
 * A wait opened: the session wants operator input for `token`. Durable:
 * the wait registry retains every unresolved wait and the session resends
 * it on reconnect, so the SPA pins its input box to the token and answers
 * with an `input_response` frame.
 */
export interface InputRequiredFrame {
  type: "input_required";
  token: string;
}

/**
 * A wait died unresolved: the prompt for `token` is stale. Durable;
 * cancellation is an outcome on the wire, never silence, so the SPA never
 * holds a prompt against a dead token.
 */
export interface InputCancelledFrame {
  type: "input_cancelled";
  token: string;
}

/** The client frame opening a session running the named agent. */
export interface LaunchFrame {
  type: "launch";
  agent: string;
}

/** The client frame reattaching to a running session after a disconnect. */
export interface AttachFrame {
  type: "attach";
  session: string;
}

/**
 * The client's answer to an `input_required` prompt: the operator's text,
 * byte-exact as typed, echoing the wait's token.
 */
export interface InputResponseFrame {
  type: "input_response";
  token: string;
  text: string;
}

/**
 * The client frame firing the session's turn-cancel. Cancellation is a
 * stop reason, never an error: the server answers with nothing, pending
 * waits die as `input_cancelled`, and the relaunched agent returns to
 * waiting.
 */
export interface AgentCancelFrame {
  type: "cancel";
}

// --- Inbound frame guards ---------------------------------------------------
// Each guard checks every field its type declares: required fields must be
// present, and optional fields, when present, must hold their type (absent
// optionals are omitted keys, never `null`). Extra fields pass, so the
// server may add fields without breaking older UIs.

type Check = (value: unknown) => boolean;
type Guard<T> = (value: unknown) => value is T;
type Checks<T> = { readonly [K in keyof T]-?: Check };

const isString: Check = (value) => typeof value === "string";
const isNumber: Check = (value) => typeof value === "number";
const isBoolean: Check = (value) => typeof value === "boolean";
const isAnything: Check = () => true;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function optional(check: Check): Check {
  return (value) => value === undefined || check(value);
}

function nullable(check: Check): Check {
  return (value) => value === null || check(value);
}

function oneOf(...labels: readonly string[]): Check {
  return (value) => typeof value === "string" && labels.includes(value);
}

function arrayOf(check: Check): Check {
  return (value) => Array.isArray(value) && value.every(check);
}

function shape<T>(checks: Checks<T>): Guard<T> {
  const entries: [string, Check][] = Object.entries(checks);
  return (value): value is T =>
    isRecord(value) && entries.every(([key, check]) => check(value[key]));
}

function frame<T extends { type: string }>(
  type: T["type"],
  checks: Omit<Checks<T>, "type">,
): Guard<T> {
  return shape<T>({ ...checks, type: (value: unknown) => value === type } as Checks<T>);
}

const isCatalogModel = shape<CatalogModel>({ id: isString, description: optional(isString) });

const isUsage = shape<Usage>({
  prompt_tokens: isNumber,
  completion_tokens: isNumber,
  total_tokens: isNumber,
  cached_tokens: optional(isNumber),
  reasoning_tokens: optional(isNumber),
});

const isLlamaTimings = shape<LlamaTimings>({
  prompt_n: isNumber,
  prompt_ms: isNumber,
  prompt_per_second: isNumber,
  predicted_n: isNumber,
  predicted_ms: isNumber,
  predicted_per_second: isNumber,
  draft_n: isNumber,
  draft_n_accepted: isNumber,
});

const isVllmMetrics = shape<VllmMetrics>({
  time_to_first_token_ms: optional(isNumber),
  generation_time_ms: optional(isNumber),
  queue_time_ms: optional(isNumber),
  mean_itl_ms: optional(isNumber),
  tokens_per_second: optional(isNumber),
});

const isClientTiming = shape<ClientTiming>({
  ttft_ms: optional(isNumber),
  mean_itl_ms: optional(isNumber),
  e2e_ms: isNumber,
});

const isCallMetrics = shape<CallMetrics>({
  usage: optional(isUsage),
  llama: optional(isLlamaTimings),
  vllm: optional(isVllmMetrics),
  client: optional(isClientTiming),
});

// `kind` stays an open string: future event kinds arrive as labels outside
// `AgentEventKind`, and renderers tolerate them.
const isAgentEvent = shape<AgentEvent>({
  kind: isString,
  section: isString,
  turn: isNumber,
  content: isString,
  model: optional(isString),
  tool_call_id: optional(isString),
  finish_reason: optional(isString),
  metrics: optional(isCallMetrics),
});

/** Guards for the frames /ws delivers, keyed by `type`. */
export const WORKSHOP_FRAME_GUARDS = {
  status: frame<StatusFrame>("status", {
    label: isString,
    description: isString,
    severity: oneOf("info", "debug", "error"),
    activity: oneOf("general", "thinking", "generating"),
    busy: isBoolean,
  }),
  models: frame<ModelsFrame>("models", { models: arrayOf(isCatalogModel) }),
  workbench: frame<WorkbenchFrame>("workbench", {
    profiles: arrayOf(isString),
    active: nullable(isString),
    switching: nullable(isString),
    switch_in_flight: isBoolean,
    selected: nullable(isString),
    chat_ready: isBoolean,
  }),
};

/** Guards for the frames /agents/ws delivers, keyed by `type`. */
export const AGENT_FRAME_GUARDS = {
  agents: frame<AgentsFrame>("agents", { agents: arrayOf(isString) }),
  agent_session: frame<AgentSessionFrame>("agent_session", { session: isString, agent: isString }),
  agent_event: frame<AgentEventFrame>("agent_event", {
    index: isNumber,
    reply: optional(isNumber),
    event: isAgentEvent,
  }),
  agent_delta: frame<AgentDeltaFrame>("agent_delta", {
    kind: oneOf("text", "reasoning"),
    content: isString,
    reply: isNumber,
  }),
  input_required: frame<InputRequiredFrame>("input_required", { token: isString }),
  input_cancelled: frame<InputCancelledFrame>("input_cancelled", { token: isString }),
  error: frame<ErrorFrame>("error", { message: isString, id: isAnything }),
};

type GuardedFrame<G> = { [K in keyof G]: G[K] extends Guard<infer T> ? T : never }[keyof G];

/**
 * Narrows one parsed inbound frame through the guard its `type` selects.
 * A frame of a guarded type that fails its guard is dropped with one
 * console warning naming the type; a frame of any other type, or JSON that
 * is no frame at all, is dropped silently.
 */
export function narrowFrame<G extends Record<string, Guard<{ type: string }>>>(
  guards: G,
  value: unknown,
  socket: string,
): GuardedFrame<G> | null {
  if (!isRecord(value) || typeof value.type !== "string" || !Object.hasOwn(guards, value.type)) {
    return null;
  }
  const guard = guards[value.type];
  if (guard?.(value)) {
    return value as GuardedFrame<G>;
  }
  console.warn(`${socket}: dropped a malformed ${value.type} frame`);
  return null;
}
