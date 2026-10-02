//! harness-web - the `promptforge/web` capability a Host registers: the
//! `promptforge/web/fetch` tool, which fetches a URL and returns its
//! content as text (markdown for an HTML page), and the
//! `promptforge/web/search` tool, which runs a search through the Host's
//! [`SearchProvider`].
//!
//! A research prompt wants both tools or neither, so the capability
//! activates as one frontmatter line (`capabilities: [promptforge/web]`).
//! The Host registers [`Web`] in its capability registry and provides two
//! services beside it: its [`SearchProvider`] under [`SEARCH_PROVIDER`],
//! and the tokio runtime handle every fetch is spawned onto under
//! [`TOKIO_RUNTIME`]. A run without either gets no web tools.
//!
//! The fetch tool is security-critical. A model supplies the URL, so the
//! tool is the SSRF boundary between an untrusted argument and the
//! network. Its whole configurable surface is one validated policy entry
//! point ([`FetchConfig`] and [`FetchConfigBuilder`]) and one opaque
//! configuration error ([`ConfigError`]); the address, resolver, redirect,
//! URL-policy, and error machinery are crate-private. It performs a GET,
//! routes the response on its `Content-Type`, and refuses a type it
//! cannot render. An HTML page has its main article content extracted
//! with [`readabilityrs`] and rendered to markdown; a page with no article
//! to extract falls back to a whole-page HTML-to-markdown conversion with
//! [`htmd`]. A non-HTML text body (JSON, XML, plain text) is returned
//! decoded, with no extraction.
//!
//! The search tool validates the model's arguments into a [`SearchQuery`],
//! hands it to the provider, and returns the [`SearchResults`] as
//! untrusted compact JSON in the Gateway's shape. The provider owns the
//! transport and its deadline, so a search vendor's credential stays
//! wherever the provider keeps it.
//!
//! ## Invariants
//!
//! - Family: Harness, at the `crates/` root beside `harness`; may depend
//!   on: `harness`, `promptforge`, and third-party crates only. Never on
//!   a `crates/harness-internal` crate, `harness-gateway-client`, or a
//!   `workshop-*`, `gateway-*`, or `shared-*` crate, and no
//!   `crates/harness-internal` crate depends on it. Read the
//!   repository-root `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Every model- or tool-selected URL and every resolved address is
//!   revalidated on each redirect hop; a non-global address is denied
//!   unless the fetch policy grants an exact host-and-address exception.
//! - No fetch includes an ambient identity on any hop: the client has
//!   no proxy, no cookie store, no automatic `Referer`, and no default
//!   credentials.
//! - Every fetch runs on the Host's runtime handle, and dropping the call
//!   aborts it.

mod address;
mod config;
mod error;
mod fetch;
mod provider;
mod redirect;
mod resolver;
mod response;
mod search;
mod url_policy;
mod web;

pub use crate::config::{ConfigError, FetchConfig, FetchConfigBuilder};
pub use crate::provider::{
    Freshness, SafeSearch, SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
    SearchResults,
};
pub use crate::web::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
