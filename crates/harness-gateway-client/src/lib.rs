//! harness-gateway-client - the standard way a Host talks to the
//! PromptForge Gateway: the OpenAI chat-completions wire code that turns a
//! `Chat` effect into a request body, a streamed reply into a
//! [`Completion`](promptforge::model::Completion), and a failed response
//! into the [`CompletionError`](promptforge::model::CompletionError) the
//! round carries.
//!
//! [`build_request_body`] builds the one JSON body every round sends.
//! [`read_completion_stream`] reads the SSE reply over a caller's
//! [`ChunkSource`] to its `[DONE]` sentinel under a byte cap, forwards each
//! live delta, and folds the stream into a completion under one strict turn
//! rule set. [`read_body_capped`] reads a body the caller decodes whole.
//! [`escape_controls`] bounds and escapes a backend error body, and
//! [`classify_http_failure`] and [`classify_stream_error`] turn a status or
//! an in-stream error envelope into a failure kind. Nothing here opens a
//! connection or reads a clock: the caller supplies the chunks and the
//! clock.
//!
//! ## Invariants
//!
//! - Family: Harness, at the `crates/` root beside `harness`; may depend
//!   on: `promptforge` and third-party crates only. Never on `harness` or
//!   any `crates/harness-internal` crate. Read the repository-root
//!   `AGENTS.md` before adding an import.
//! - Every `Completion` and `ToolCall` is built through the public
//!   validating constructors, so the Engine's neutral reply checks run on
//!   every decoded turn.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

mod failure;
mod wire;

pub use wire::classify::classify_http_failure;
pub use wire::classify::classify_stream_error;
pub use wire::read::ChunkSource;
pub use wire::read::read_body_capped;
pub use wire::read::read_completion_stream;
pub use wire::request::build_request_body;
pub use wire::stream::escape_controls;
