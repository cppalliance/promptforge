//! The `web_search` tool: validate a search's arguments, run it through
//! the Host's [`SearchProvider`], and return the results as untrusted
//! compact JSON in the Gateway's shape.

use std::fmt;
use std::sync::Arc;

use promptforge_plugin::{ToolError, ToolErrorKind, ToolOutput};

use crate::provider::{SearchErrorKind, SearchProvider};

#[path = "search-request.rs"]
mod request;

use request::SearchRequest;

/// The largest accepted `query` string, in characters (Brave's documented cap).
const MAX_QUERY_LEN: usize = 400;
/// The inclusive upper bound on the requested result `count`.
const MAX_COUNT: u32 = 20;
/// The largest accepted free-form string argument (country, language, domain).
const MAX_STRING_LEN: usize = 128;
/// The largest number of hostnames accepted in a domain include/exclude list.
const MAX_DOMAINS: usize = 20;

/// A tool that searches the web through the Host's [`SearchProvider`].
///
/// Each call validates its arguments, hands the validated query to the
/// provider, and returns the results as untrusted output. The deadline is
/// the provider's: the tool sets none of its own.
#[derive(Clone)]
pub(crate) struct WebSearch {
    /// The Host's provider every search runs through.
    provider: Arc<dyn SearchProvider>,
}

impl fmt::Debug for WebSearch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("WebSearch").finish_non_exhaustive()
    }
}

impl WebSearch {
    /// Builds the tool over the Host's `provider`.
    pub(crate) fn new(provider: Arc<dyn SearchProvider>) -> WebSearch {
        WebSearch { provider }
    }

    /// The one sentence the model reads to decide when to search.
    #[expect(
        clippy::unused_self,
        reason = "both tools' descriptions and schemas are read through the tool, as the fetch tool's schema reads its policy"
    )]
    pub(crate) fn description(&self) -> &'static str {
        // Keep this sentence aligned with shipped prompts/picker fixtures; knobs
        // live in parameters_schema so capability bind stays stable.
        "Search the web and return a list of results (title, url, description)."
    }

    /// The JSON Schema for a search's arguments.
    #[expect(
        clippy::unused_self,
        reason = "both tools' descriptions and schemas are read through the tool, as the fetch tool's schema reads its policy"
    )]
    pub(crate) fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query.",
                    "minLength": 1,
                    "maxLength": MAX_QUERY_LEN
                },
                "count": {
                    "type": "integer",
                    "description": "Max number of results.",
                    "minimum": 1,
                    "maximum": MAX_COUNT
                },
                "freshness": {
                    "type": "string",
                    "description": "Freshness filter.",
                    "enum": ["pd", "pw", "pm", "py"]
                },
                "country": {
                    "type": "string",
                    "description": "Country code for the search.",
                    "maxLength": MAX_STRING_LEN
                },
                "search_lang": {
                    "type": "string",
                    "description": "Search language code.",
                    "maxLength": MAX_STRING_LEN
                },
                "safesearch": {
                    "type": "string",
                    "description": "SafeSearch level.",
                    "enum": ["off", "moderate", "strict"]
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Only keep results from these hostnames."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Drop results from these hostnames."
                }
            },
            "required": ["query"]
        })
    }

    /// Validates `args`, runs the search through the provider, and returns
    /// the results as untrusted compact JSON.
    ///
    /// # Errors
    /// Returns a [`ToolError`] for invalid arguments, a provider failure,
    /// or a malformed response.
    pub(crate) async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Validate and normalize arguments before the provider spends a
        // round; only the validated query reaches it.
        let query = SearchRequest::from_args(args)?.into_query();

        let results = self.provider.search(query).await.map_err(|error| {
            let kind = match error.kind() {
                SearchErrorKind::Transport => ToolErrorKind::Transport,
                SearchErrorKind::Backend => ToolErrorKind::Backend,
            };
            ToolError::with_source(format!("web_search: {error}"), error).with_kind(kind)
        })?;
        if let Some(index) = results.results.iter().position(|r| r.url.trim().is_empty()) {
            return Err(ToolError::message(format!(
                "web_search: malformed search response: result {index} has an empty url"
            ))
            .with_kind(ToolErrorKind::Backend));
        }
        let body = serde_json::to_string(&results).map_err(|source| {
            ToolError::with_source("web_search: the results could not be rendered", source)
                .with_kind(ToolErrorKind::Other)
        })?;

        // The results embed third-party titles, URLs, and descriptions, so
        // the body is marked untrusted: it is nonce-wrapped before it can
        // reach model input.
        Ok(ToolOutput::untrusted(body))
    }
}

#[cfg(test)]
#[path = "search-tests.rs"]
mod tests;
