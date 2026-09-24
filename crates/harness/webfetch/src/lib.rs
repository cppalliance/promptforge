//! The `web_fetch` tool: fetch a URL and return its content as text, markdown
//! for an HTML page.
//!
//! This crate is security-critical. A model supplies the URL, so the crate is
//! the SSRF boundary between an untrusted argument and the network. The whole
//! supported surface is [`WebFetch`] plus one validated configuration entry
//! point ([`FetchConfig`] and [`FetchConfigBuilder`]) and one opaque
//! configuration error ([`ConfigError`]); the address, resolver, redirect,
//! URL-policy, and error machinery are crate-private implementation details.
//!
//! `WebFetch` performs a GET, routes the response on its `Content-Type`, and
//! refuses a type it cannot render. An HTML page has its main article
//! content extracted with [`readabilityrs`] and rendered to markdown; a page
//! with no article to extract falls back to a whole-page HTML-to-markdown
//! conversion with [`htmd`]. A non-HTML text body (JSON, XML, plain text) is
//! returned decoded, with no extraction.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge`, `gateway-api-types`, `gateway-api-discovery`,
//!   `shared-*`, and its container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Every model- or tool-selected URL and every resolved address is
//!   revalidated on each redirect hop; a non-global address is denied
//!   unless the fetch policy grants an exact host-and-address exception.
//! - No request includes an ambient identity on any hop: the client has
//!   no proxy, no cookie store, no automatic `Referer`, and no default
//!   credentials.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod address;
mod config;
mod error;
mod redirect;
mod resolver;
mod response;
mod tool;
mod url_policy;

pub use crate::config::{ConfigError, FetchConfig, FetchConfigBuilder};
pub use crate::tool::WebFetch;
