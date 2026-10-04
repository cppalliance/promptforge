//! The standard client a Host uses to run model rounds, list models, and
//! search the web through the PromptForge Gateway.
//!
//! [`GatewayBroker`] is the [`harness::InferenceBroker`] that a Host using
//! the Gateway passes to `Harness::new`. It runs every model round on a
//! [`GatewayChat`] under the Engine's default run limits, and it lists the
//! Gateway's models through [`fetch_model_catalog`]. A Host that shows a
//! reply as it forms runs the round through
//! [`GatewayBroker::chat_streaming`] instead, which hands each
//! [`StreamDelta`] to the Host's callback as it arrives.
//!
//! [`GatewayChat`] is the HTTP client that sends a `Chat` effect's round
//! to one Gateway URL. It presents the Gateway's shared bearer key, or no
//! key when built keyless. Every request streams: the Gateway's
//! `/chat/completions` endpoint answers with server-sent events (SSE).
//! [`GatewayChat::complete`] sends the request body and reads the stream
//! within the client's byte cap and per-receive timeout. It calls the
//! caller's delta callback as each piece arrives and returns the one
//! completion the round produced. [`fetch_model_catalog`] fetches the
//! Gateway's model list as a typed `ModelCatalog`. The client holds no
//! credential except the Gateway's shared key. The model vendor's
//! credential stays in the Gateway.
//!
//! [`GatewaySearch`] is the [`harness_web::SearchProvider`] that a Host
//! using the Gateway supplies for web search. It sends each search through
//! the Gateway's `/tools/web_search` relay with a 30-second deadline and
//! maps the reply into the provider's results. The search vendor's
//! credential also stays in the Gateway.
//!
//! Under the client sits the OpenAI chat-completions wire code. It turns a
//! round into a request body, a streamed reply into a
//! [`Completion`](promptforge::model::Completion), and a failed response
//! into the [`CompletionError`] the round fails with.
//!
//! - [`build_request_body`] builds the JSON body that every round sends.
//! - [`read_completion_stream`] reads a streamed reply from a
//!   [`ChunkSource`] the caller supplies, up to its `[DONE]` sentinel and
//!   within a byte cap. It forwards each live delta, assembles the stream
//!   into one completion, and checks that turn against one strict set of
//!   rules.
//! - [`read_body_capped`] reads, within a byte cap, a body that the caller
//!   decodes whole.
//! - [`escape_controls`] cuts a backend error body to a length limit and
//!   escapes its control characters.
//! - [`classify_http_failure`] turns a failure status and its body into a
//!   `CompletionError` of the matching kind. [`classify_stream_error`] does
//!   the same for an error envelope that arrives inside the stream.
//!
//! The wire code opens no connection and reads no clock. The client, or
//! another broker, supplies the chunks and the clock.
//!
//! ## Invariants
//!
//! - Every `Completion` and `ToolCall` is built through the Engine's public
//!   validating constructors, so the Engine's model-independent reply
//!   checks run on every decoded turn.
//! - A Gateway bearer key is never written to logs, `Debug`, `Display`, or
//!   error text.
//! - A backend error body is bounded and control-escaped before it is
//!   kept. A chat round keeps it only in the opt-in
//!   `CompletionError::detail`, and a search keeps it in the
//!   `GatewaySearchError` message.
//! - A client sends requests without a key only when the caller builds it
//!   with `GatewayChat::keyless`, which does not check the endpoint's
//!   address, or when `GatewayChat::from_env` finds no key for a loopback
//!   URL. A client built with `GatewayChat::disabled` also holds no key,
//!   but it sends no requests.

mod broker;
mod catalog;
mod config;
mod failure;
mod search;
mod transport;
mod wire;

pub use broker::GatewayBroker;
pub use catalog::fetch_model_catalog;
pub use config::GatewayConfigError;
pub use config::GatewayEndpoint;
pub use config::SecretError;
pub use config::SecretString;
pub use promptforge::model::CompletionError;
pub use promptforge::model::CompletionErrorKind;
pub use search::GatewaySearch;
pub use search::GatewaySearchError;
pub use search::GatewaySearchErrorKind;
pub use transport::GatewayChat;
pub use wire::classify::classify_http_failure;
pub use wire::classify::classify_stream_error;
pub use wire::delta::StreamDelta;
pub use wire::read::ChunkSource;
pub use wire::read::read_body_capped;
pub use wire::read::read_completion_stream;
pub use wire::request::build_request_body;
pub use wire::stream::escape_controls;
