//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This crate is the concrete search provider. It POSTs the query to the
//! gateway's `POST /v1/tools/web_search` endpoint with the shared bearer
//! token instead of calling a search vendor directly, so the vendor
//! credential never leaves the server. The gateway's JSON results are
//! validated for shape and returned as untrusted output, ready to hand
//! back to the model.
//!
//! The whole supported surface is [`WebSearch`]; the endpoint validation and
//! the redacted bearer token are crate-private implementation details. The
//! tool vocabulary ([`Tool`](harness_capabilities::Tool),
//! [`ToolError`](promptforge::tools::ToolError), and their kinds)
//! comes from `promptforge`.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - The gateway bearer token is never written to logs or `Debug` output;
//!   only the request builder reads it, to set the `Authorization` header.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod endpoint;
mod secret;
mod web_search;

pub use crate::web_search::WebSearch;
