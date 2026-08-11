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
//! refuses a type it cannot render. An HTML page has its main article content
//! extracted with [`readabilityrs`] and rendered to markdown; a page that is not
//! article-shaped falls back to a whole-page HTML-to-markdown conversion with
//! [`htmd`]. A non-HTML text body (JSON, XML, plain text) is returned decoded,
//! with no extraction.

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
