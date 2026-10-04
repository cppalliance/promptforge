//! The search provider a Host supplies: the [`SearchProvider`] trait, the
//! validated [`SearchQuery`] the search tool hands it, the
//! [`SearchResults`] it answers with, and the [`SearchError`] it fails
//! with.

/// Runs one web search for the `promptforge/web/search` tool.
///
/// The Host provides one under [`SEARCH_PROVIDER`](crate::SEARCH_PROVIDER).
/// The tool validates the model's arguments before it calls the provider,
/// and sets no deadline of its own: the provider bounds its own round.
#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    /// Runs `query` and returns its results.
    ///
    /// # Errors
    /// Returns a [`SearchError`] whose kind says whether the transport or
    /// the backend failed. Its message reaches the model after a
    /// `web_search: ` prefix, so it must hold no credential.
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError>;
}

/// The validated arguments of one `promptforge/web/search` call.
///
/// The search tool builds one only from arguments that passed its checks:
/// a non-blank `query` of at most 400 characters, a `count` in `1..=20`,
/// a `country` and `search_lang` of 1 to 128 characters, and at most 20
/// hostnames in each domain list. An empty domain list filters nothing.
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

/// The freshness filter of a [`SearchQuery`], deserialized from the
/// model's arguments as a closed enum so an unknown token is refused as an
/// invalid argument rather than forwarded.
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
    /// The filter's token: `pd`, `pw`, `pm`, or `py`.
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

/// The SafeSearch level of a [`SearchQuery`], deserialized as a closed
/// enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SafeSearch {
    /// No filtering.
    Off,
    /// Moderate filtering.
    Moderate,
    /// Strict filtering.
    Strict,
}

impl SafeSearch {
    /// The level's token: `off`, `moderate`, or `strict`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SafeSearch::Off => "off",
            SafeSearch::Moderate => "moderate",
            SafeSearch::Strict => "strict",
        }
    }
}

/// What a [`SearchProvider`] answers: the query it ran and its results.
///
/// The search tool renders it as compact JSON in the Gateway's field
/// order and leaves out an absent `age` or `site_name` and an empty
/// `extra_snippets`, as the Gateway does.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SearchResults {
    /// The query the provider ran.
    pub query: String,
    /// The results, in the provider's order.
    pub results: Vec<SearchResult>,
}

/// One row of [`SearchResults`]. The search tool refuses a reply holding
/// a row whose `url` is blank.
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

/// Which side of a search failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchErrorKind {
    /// The request could not be sent or its reply could not be read,
    /// timeouts included.
    Transport,
    /// The backend answered with a failure or an unusable reply.
    Backend,
}

/// Why a [`SearchProvider`] failed: its kind, a message, and the cause
/// when there is one.
///
/// The search tool keeps the whole error as its own error's source, so
/// the cause survives the provider boundary.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SearchError {
    kind: SearchErrorKind,
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl SearchError {
    /// Builds an error with no cause.
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

    /// Returns which side of the search failed.
    #[must_use]
    pub fn kind(&self) -> SearchErrorKind {
        self.kind
    }
}
