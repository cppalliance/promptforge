//! The gateway-side web-search service: the Brave Search provider client,
//! request validation, and result post-processing behind [`WebSearchState`].
//!
//! The gateway owns the `POST /v1/tools/web_search` route, the bearer-auth
//! check, and the profile switch reload; it builds a [`WebSearchState`] from
//! the active profile's `[tools.web_search]` section and calls
//! [`WebSearchState::search`]. The provider credential is held in a
//! [`gateway_config::Secret`], which redacts in `Debug` and
//! `Display`, and leaves this crate only at the provider call site.

mod brave;
mod error;
mod process;
mod service;

pub use crate::error::WebSearchError;
pub use crate::service::{SearchResult, WebSearchRequest, WebSearchResponse, WebSearchState};
