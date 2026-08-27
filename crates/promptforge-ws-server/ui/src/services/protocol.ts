// The pure wire types of the workshop protocol: the JSON frame and payload
// shapes exchanged with the server over /ws and /v1/models. Types only -
// the socket logic that sends and routes these frames stays in
// workshop-socket.ts. The Rust half of this contract is
// crates/promptforge-ws-server/src/protocol.rs (created in a later refactor
// step); the two files cross-cite each other so a shape change touches both
// or neither.

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

/** The chat payload sent upstream in one `{"type":"chat",...}` frame. */
export interface ChatPayload {
  model: string;
  messages: Array<{ role: string; content: string }>;
}
