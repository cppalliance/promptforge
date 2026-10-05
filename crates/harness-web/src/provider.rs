//! The search provider a Host supplies: the [`SearchProvider`] trait, the
//! validated [`SearchQuery`] the search tool hands it, the
//! [`SearchResults`] it answers with, and the [`SearchError`] it fails
//! with.

/// A backend that runs web searches for the `promptforge/web/search` tool.
///
/// The Host registers one under the key
/// [`SEARCH_PROVIDER`](crate::SEARCH_PROVIDER). The tool validates the
/// model's arguments before it calls the provider. The tool leaves the
/// deadline to the provider, which must limit how long each search takes.
#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    /// Runs `query` and returns its results.
    ///
    /// # Errors
    /// Returns a [`SearchError`] whose kind says whether the transport or
    /// the backend failed. The model sees the error's message after a
    /// `web_search: ` prefix, so the message must not contain a credential.
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError>;
}

/// The validated arguments of one `promptforge/web/search` call.
///
/// The search tool builds one only from arguments that pass its checks:
///
/// - `query` holds more than whitespace and has at most 400 characters.
/// - `count`, when given, is in `1..=20`.
/// - `country` and `search_lang`, when given, have 1 to 128 characters.
/// - Each domain list holds at most 20 hostnames.
///
/// An empty domain list keeps every result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// The search query.
    pub query: String,
    /// The number of results wanted.
    pub count: Option<u8>,
    /// The freshness filter.
    pub freshness: Option<Freshness>,
    /// The country code for the search.
    pub country: Option<String>,
    /// The search language code.
    pub search_lang: Option<String>,
    /// The SafeSearch level.
    pub safesearch: Option<SafeSearch>,
    /// Keep only results from these hostnames.
    pub include_domains: Vec<String>,
    /// Drop results from these hostnames.
    pub exclude_domains: Vec<String>,
}

/// How recent the results of a [`SearchQuery`] must be.
///
/// The search tool reads it from the model's arguments. It refuses any
/// token other than `pd`, `pw`, `pm`, or `py` as an invalid argument, so
/// that token never reaches the provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Freshness {
    /// Past day.
    Pd,
    /// Past week.
    Pw,
    /// Past month.
    Pm,
    /// Past year.
    Py,
}

impl Freshness {
    /// Returns the filter's token: `pd`, `pw`, `pm`, or `py`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Pd => "pd",
            Freshness::Pw => "pw",
            Freshness::Pm => "pm",
            Freshness::Py => "py",
        }
    }
}

/// The SafeSearch filtering level of a [`SearchQuery`].
///
/// The search tool reads it from the model's arguments. It refuses any
/// token other than `off`, `moderate`, or `strict` as an invalid argument,
/// so that token never reaches the provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SafeSearch {
    /// Filtering turned off.
    Off,
    /// Moderate filtering.
    Moderate,
    /// Strict filtering.
    Strict,
}

impl SafeSearch {
    /// Returns the level's token: `off`, `moderate`, or `strict`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SafeSearch::Off => "off",
            SafeSearch::Moderate => "moderate",
            SafeSearch::Strict => "strict",
        }
    }
}

/// The results a [`SearchProvider`] returns for one search, with the query
/// it ran.
///
/// The search tool returns it to the model as compact JSON: an object with
/// `query` and a `results` array. The fields appear in the same order as
/// in the Gateway's search output. Like the Gateway, the JSON includes a
/// result's `age` and `site_name` only when present, and its
/// `extra_snippets` only when it holds at least one snippet.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SearchResults {
    /// The query the provider ran.
    pub query: String,
    /// The results, in the provider's order.
    pub results: Vec<SearchResult>,
}

/// One result in [`SearchResults`]: a title, a URL, a description, and
/// optional extras.
///
/// The search tool rejects the provider's whole reply when any result has
/// a blank `url`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    /// The result's title.
    pub title: String,
    /// The result's URL.
    pub url: String,
    /// A short description or snippet.
    pub description: String,
    /// The result's age, when the provider reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<String>,
    /// The hostname of `url`, when the provider derives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// Extra snippets from the provider.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_snippets: Vec<String>,
}

/// Which part of a search failed: the transport or the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchErrorKind {
    /// Sending the request or reading its reply failed. A timeout counts as
    /// this kind.
    Transport,
    /// The backend answered with a failure or with a reply the provider
    /// rejects.
    Backend,
}

/// The error a [`SearchProvider`] returns when a search fails.
///
/// It holds a kind, a message, and an optional cause. The search tool
/// keeps the whole error as the source of the tool error it returns, so
/// the cause stays in the error chain.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SearchError {
    kind: SearchErrorKind,
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl SearchError {
    /// Builds an error from a kind and a message alone.
    #[must_use]
    pub fn new(kind: SearchErrorKind, message: impl Into<String>) -> SearchError {
        SearchError {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Builds an error whose cause is `source`.
    #[must_use]
    pub fn with_source(
        kind: SearchErrorKind,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> SearchError {
        SearchError {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Returns which part of the search failed.
    #[must_use]
    pub fn kind(&self) -> SearchErrorKind {
        self.kind
    }
}
