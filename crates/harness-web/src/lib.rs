//! Web access for prompts: a Plugin that gives a run two tools, one
//! that fetches a page and one that searches the web.
//!
//! A Host registers [`Web`] in its Plugin registry. A prompt turns it
//! on with one frontmatter line, `plugins: [promptforge/web]`. The
//! run then gets both tools: `promptforge/web/fetch`, which fetches a URL
//! and returns its content as text, and `promptforge/web/search`, which
//! runs a search through the Host's [`SearchProvider`]. The two tools
//! always come together.
//!
//! The Host also provides two services beside the Plugin. Its
//! [`SearchProvider`] is registered under the key [`SEARCH_PROVIDER`]. The
//! tokio runtime handle that every fetch is spawned onto is registered
//! under the key [`TOKIO_RUNTIME`]. A run gets the web tools only when
//! both services are registered.
//!
//! The fetch tool is security-critical. The model supplies the URL, so the
//! tool is the server-side request forgery (SSRF) boundary between an
//! untrusted argument and the network. A Host can replace the default
//! fetch policy with a [`FetchConfig`] built by [`FetchConfigBuilder`].
//! A configuration problem is reported as a [`ConfigError`].
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
//! - Every URL selected by the model or a tool, and every resolved
//!   address, is validated again on each redirect hop. A non-global
//!   address is denied unless the fetch policy grants an exact
//!   host-and-address exception.
//! - No fetch includes an ambient identity on any hop. The client has no
//!   proxy, no cookie store, no automatic `Referer` header, and no default
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
#[cfg(test)]
mod test_support;
mod url_policy;
mod web;

pub use crate::config::{ConfigError, FetchConfig, FetchConfigBuilder};
pub use crate::provider::{
    Freshness, SafeSearch, SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
    SearchResults,
};
pub use crate::web::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
