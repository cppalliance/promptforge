//! The validated search request: the `deny_unknown_fields` argument shape,
//! the closed freshness and SafeSearch enums, and the bounds the type alone
//! cannot express. Only a value that passed [`SearchRequest::from_args`]
//! becomes the provider's [`SearchQuery`].

use promptforge_plugin::{ToolError, ToolErrorKind};

use super::{MAX_COUNT, MAX_DOMAINS, MAX_QUERY_LEN, MAX_STRING_LEN};
use crate::provider::{Freshness, SafeSearch, SearchQuery};

/// The search request as the model sent it, once it deserialized.
///
/// `deny_unknown_fields` means an argument the tool does not model is rejected
/// (rather than silently forwarded), and the typed optional fields reject a
/// wrong JSON type at deserialization. [`SearchRequest::validate`] then enforces
/// the string, count, and domain bounds.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchRequest {
    /// The search query.
    query: String,
    /// Maximum number of results.
    #[serde(default)]
    count: Option<u32>,
    /// Freshness filter.
    #[serde(default)]
    freshness: Option<Freshness>,
    /// Country code for the search.
    #[serde(default)]
    country: Option<String>,
    /// Search language code.
    #[serde(default)]
    search_lang: Option<String>,
    /// SafeSearch level.
    #[serde(default)]
    safesearch: Option<SafeSearch>,
    /// Only keep results from these hostnames.
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    /// Drops results from these hostnames.
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
}

impl SearchRequest {
    /// Deserializes and validates the raw call arguments.
    pub(super) fn from_args(args: serde_json::Value) -> Result<SearchRequest, ToolError> {
        let request: SearchRequest = serde_json::from_value(args).map_err(|error| {
            ToolError::with_source("web_search: invalid arguments", error)
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        request.validate()?;
        Ok(request)
    }

    /// The validated request as the provider's query. `validate` bounded
    /// `count` to `1..=MAX_COUNT`, so it fits a `u8`.
    pub(super) fn into_query(self) -> SearchQuery {
        SearchQuery {
            query: self.query,
            count: self.count.and_then(|count| u8::try_from(count).ok()),
            freshness: self.freshness,
            country: self.country,
            search_lang: self.search_lang,
            safesearch: self.safesearch,
            include_domains: self.include_domains.unwrap_or_default(),
            exclude_domains: self.exclude_domains.unwrap_or_default(),
        }
    }

    /// Enforces the bounds the type alone cannot express.
    fn validate(&self) -> Result<(), ToolError> {
        let invalid = |message: String| {
            ToolError::message(message).with_kind(ToolErrorKind::InvalidArguments)
        };
        if self.query.trim().is_empty() {
            return Err(invalid("web_search: query must not be empty".to_owned()));
        }
        if self.query.chars().count() > MAX_QUERY_LEN {
            return Err(invalid(format!(
                "web_search: query exceeds {MAX_QUERY_LEN} characters"
            )));
        }
        if let Some(count) = self.count
            && !(1..=MAX_COUNT).contains(&count)
        {
            return Err(invalid(format!(
                "web_search: count must be between 1 and {MAX_COUNT}"
            )));
        }
        for (field, value) in [
            ("country", &self.country),
            ("search_lang", &self.search_lang),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty() || value.chars().count() > MAX_STRING_LEN)
            {
                return Err(invalid(format!(
                    "web_search: {field} must be 1..={MAX_STRING_LEN} characters"
                )));
            }
        }
        for (field, domains) in [
            ("include_domains", &self.include_domains),
            ("exclude_domains", &self.exclude_domains),
        ] {
            if let Some(domains) = domains {
                if domains.len() > MAX_DOMAINS {
                    return Err(invalid(format!(
                        "web_search: {field} may list at most {MAX_DOMAINS} hostnames"
                    )));
                }
                for domain in domains {
                    let bad = domain.trim().is_empty()
                        || domain.chars().count() > MAX_STRING_LEN
                        || domain.contains('/')
                        || domain.chars().any(|c| c.is_whitespace() || c.is_control());
                    if bad {
                        return Err(invalid(format!(
                            "web_search: {field} contains an invalid hostname"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}
