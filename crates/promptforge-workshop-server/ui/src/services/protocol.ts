// The pure wire types of the workshop protocol: the JSON frame and payload
// shapes exchanged with the server over /ws, /agents/ws, /voice, and
// /v1/models. Types only - the socket logic that sends and routes these
// frames stays in workshop-socket.ts, agent-socket.ts, and ui/voice.ts. The
// Rust half of this contract is
// crates/promptforge-workshop-server/src/protocol.rs; the two files
// cross-cite each other so a shape change touches both or neither. The
// agent-session frame family is additionally pinned by the shared fixture
// crates/promptforge-workshop-server/tests/fixtures/agent-frames.json,
// asserted as the same JSON by both suites (test/agent-wire-fixtures.mjs
// here, the protocol.rs fixture test there), so drift on either side fails
// that side's tests.

/** One observer status update, as sent by the server. */
export interface StatusFrame {
  type: "status";
  label: string;
  description: string;
  severity: "info" | "debug" | "error";
  activity: "general" | "thinking" | "generating";
  progress: { current: number; total: number } | null;
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
 * the UI never derives it.
 */
export interface WorkbenchFrame {
  type: "workbench";
  profiles: string[];
  active: string | null;
  switching: string | null;
  selected: string | null;
  chat_ready: boolean;
}

/** The chat payload sent upstream in one `{"type":"chat",...}` frame. */
export interface ChatPayload {
  model: string;
  messages: Array<{ role: string; content: string }>;
}

// --- Agent-session frames (/agents/ws) --------------------------------------
// The Rust half of this family is the frame structs in
// crates/promptforge-workshop-server/src/protocol.rs and the routing in
// src/session_agents/socket.rs. Delivery classes mirror the Rust docs:
// durable frames deliver exactly (the event log's per-client cursor and the
// wait registry's resend-on-attach are the repair paths), ephemeral frames
// may drop under lag and repair from a complete snapshot or a superseding
// durable event.

/**
 * The kind of one runtime event, following the Agent Client Protocol
 * `sessionUpdate` names. Mirrors `RuntimeEventKind` in
 * promptforge-core-support (src/events.rs), which is `#[non_exhaustive]`:
 * future kinds (`plan`, tool-status updates) may arrive as labels outside
 * this union, so renderers matching on kinds tolerate unknown labels
 * through a wildcard arm.
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
 * mirroring `RuntimeEvent` in promptforge-core-support (src/events.rs).
 * `content` and every other free-text field is untrusted model-, tool-, or
 * user-authored data. Absent optional fields are omitted keys on the wire,
 * never `null`.
 */
export interface RuntimeEvent {
  kind: AgentEventKind;
  /** The reporting scope: for agent sessions, the agent's name. */
  section: string;
  chain_id: number;
  depth: number;
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
  event: RuntimeEvent;
}

/** Which streaming side channel one agent delta belongs to. */
export type AgentDeltaKind = "text" | "reasoning";

/**
 * One live streaming chunk of an agent's model round. Ephemeral: deltas
 * ride a bounded broadcast and may drop under lag; the completed-reply
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

/**
 * The /voice announcement that a `start` began a new stream generation,
 * sent before any of that generation's interim or final frames.
 * Generations count from 1 per connection; the client tracks the current
 * one and discards frames a stop/restart race left behind from a
 * superseded take.
 */
export interface StreamFrame {
  type: "stream";
  generation: number;
}

/**
 * One interim transcription push on /voice: the take's crystallized
 * committed prefix (append-only within a take) plus the interim model's
 * decode of the audio past it, tagged with the take's stream generation.
 */
export interface InterimFrame {
  type: "interim";
  committed: string;
  tentative: string;
  generation: number;
}

/**
 * The take's single stop reply on /voice: the assembled transcript plus
 * the total PCM frames received since the take's start, tagged with the
 * take's stream generation.
 */
export interface FinalFrame {
  type: "final";
  text: string;
  frames: number;
  generation: number;
}
