//! Web access for prompts: a Plugin that gives a run two tools, one
//! that fetches a page and one that searches the web.
//!
//! A Host installs [`PACKAGE`], by default under the name `web`, and every
//! run then receives both tools: `web/fetch`, which fetches a URL and
//! returns its content as text, and `web/search`, which runs a search
//! through the Host's [`SearchProvider`]. The two tools always come
//! together, named under whatever name the Host installed the Plugin
//! under.
//!
//! The Plugin has no prelude, so declaring it, as in `plugins: [web]`,
//! only makes it required. A prompt reaches the tools by binding one in
//! its `tools:` frontmatter, as in `fetch: web/fetch`, or, when it does
//! not declare the Plugin, by adding them from `tools.offered()`.
//!
//! The Host provides two Host-wide services beside the Plugin. Its
//! [`SearchProvider`] goes under the key [`SEARCH_PROVIDER`], and the
//! tokio runtime handle that every fetch is spawned onto goes under the
//! key [`TOKIO_RUNTIME`]. Install reads both; when either is missing, the
//! Plugin is installed as unavailable, no run receives its tools, and a
//! prompt that declares it or binds one of its tools is refused naming
//! the missing service.
//!
//! The fetch tool is security-critical. The model supplies the URL, so the
//! tool is the server-side request forgery (SSRF) boundary between an
//! untrusted argument and the network. It enforces the built-in default
//! fetch policy.
//!
//! The fetch tool sends a GET request and chooses how to render the
//! response from its `Content-Type`. For an HTML page, it extracts the
//! main article with `readabilityrs` and renders it to markdown. When a
//! page yields too little article text, `htmd` converts the whole page to
//! markdown. Any other text body, such as JSON, XML, or plain text, is
//! decoded and returned verbatim. The tool refuses every other type.
//!
//! The search tool validates the model's arguments into a [`SearchQuery`]
//! and hands it to the provider. It returns the provider's
//! [`SearchResults`] as compact JSON, marked untrusted. The JSON matches
//! the Gateway's search output: an object with the `query` and a `results`
//! array. Each result has a `title`, `url`, and `description`, plus `age`,
//! `site_name`, and `extra_snippets` when present. The provider owns the
//! transport and its deadline, so a search vendor's credential stays
//! wherever the provider keeps it.
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-plugin`, `shared-*` crates,
//!   `workspace-hack`, and outside libraries. `cargo test -p build-xtask`
//!   enforces the Plugin family's allow-list.
//! - Every URL selected by the model or a tool, and every resolved
//!   address, is validated again on each redirect hop. A non-global
//!   address is denied unless the fetch policy grants an exact
//!   host-and-address exception.
//! - No fetch includes an ambient identity on any hop. The client has no
//!   proxy, no cookie store, no automatic `Referer` header, and no default
//!   credentials.
//! - Every fetch runs on the Host's runtime handle, and dropping the call
//!   aborts it.
//! - Install accepts only a `null` or `{}` configuration; any other value
//!   leaves the Plugin unavailable.

mod address;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "web's configuration accepts no settings yet, so only the crate's tests build a custom fetch policy"
    )
)]
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

pub use crate::provider::{
    Freshness, SafeSearch, SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
    SearchResults,
};
pub use crate::web::{PACKAGE, SEARCH_PROVIDER, TOKIO_RUNTIME};
