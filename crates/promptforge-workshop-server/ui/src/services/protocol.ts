// The pure wire types of the workshop protocol: the JSON frame and payload
// shapes exchanged with the server over /ws, /voice, and /v1/models. Types
// only - the socket logic that sends and routes these frames stays in
// workshop-socket.ts and ui/voice.ts. The Rust half of this contract is
// crates/promptforge-workshop-server/src/protocol.rs; the two files cross-cite
// each other so a shape change touches both or neither.

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
