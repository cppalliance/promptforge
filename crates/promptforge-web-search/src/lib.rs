//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This crate is the concrete search provider. It does not talk to a search
//! vendor directly; it POSTs the query to the gateway's
//! `POST /v1/tools/web_search` endpoint with the shared bearer token, so the
//! vendor credential never leaves the server. The gateway's JSON results are
//! validated for shape and returned as untrusted output, ready to hand back to
//! the model.
//!
//! The whole supported surface is [`WebSearch`]; the endpoint validation and
//! the redacted bearer token are crate-private implementation details. The
//! tool vocabulary ([`Tool`](promptforge_tools::Tool),
//! [`ToolError`](promptforge_tools::ToolError), and their kinds) comes from
//! `promptforge-tools`.

mod endpoint;
mod secret;
mod web_search;

pub use crate::web_search::WebSearch;
