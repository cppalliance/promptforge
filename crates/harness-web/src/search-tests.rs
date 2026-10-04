//! Tests for the `WebSearch` tool against a fake provider: descriptor,
//! argument validation, the query the provider receives, the rendered
//! results, and how a provider failure maps.

use std::sync::{Arc, Mutex};

use super::{MAX_COUNT, MAX_DOMAINS, MAX_QUERY_LEN, MAX_STRING_LEN, WebSearch};
use crate::provider::{
    Freshness, SafeSearch, SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
    SearchResults,
};
use crate::test_support::TestContext;
use harness::plugin::Tool;
use promptforge::tools::{OutputTrust, ToolError, ToolErrorKind, ToolId};

/// A provider that records every query and answers each with `reply`.
struct Fake {
    reply: fn() -> Result<SearchResults, SearchError>,
    queries: Mutex<Vec<SearchQuery>>,
}

#[async_trait::async_trait]
impl SearchProvider for Fake {
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError> {
        self.queries.lock().unwrap().push(query);
        (self.reply)()
    }
}

impl Fake {
    fn queries(&self) -> Vec<SearchQuery> {
        self.queries.lock().unwrap().clone()
    }
}

/// The tool over a fake answering `reply`, and the fake.
fn tool_answering(reply: fn() -> Result<SearchResults, SearchError>) -> (WebSearch, Arc<Fake>) {
    let fake = Arc::new(Fake {
        reply,
        queries: Mutex::new(Vec::new()),
    });
    (WebSearch::new(fake.clone()), fake)
}

/// One result with a title and URL.
fn one_result() -> SearchResults {
    SearchResults {
        query: "hi".to_owned(),
        results: vec![SearchResult {
            title: "T".to_owned(),
            url: "https://e.com".to_owned(),
            description: "D".to_owned(),
            ..SearchResult::default()
        }],
    }
}

/// The tool over a provider that answers one result.
fn tool() -> (WebSearch, Arc<Fake>) {
    tool_answering(|| Ok(one_result()))
}

/// Calls the tool with `args`, expecting a refusal, and checks the
/// provider was never asked.
async fn refused(args: serde_json::Value) -> ToolError {
    let (tool, fake) = tool();
    let err = tool
        .call(TestContext::new().lend(), args)
        .await
        .expect_err("invalid arguments must be refused");
    assert!(
        fake.queries().is_empty(),
        "invalid arguments never reach the provider"
    );
    err
}

#[test]
fn descriptor_is_stable_and_faithful() {
    let (tool, _fake) = tool();

    assert_eq!(
        tool.id(),
        ToolId::parse("promptforge/web/search").expect("valid id")
    );
    assert_eq!(tool.wire_name(), "web_search");
    assert_eq!(
        tool.description(),
        "Search the web and return a list of results (title, url, description)."
    );
    assert_eq!(
        tool.parameters_schema(),
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
    );
}

#[test]
fn the_migrated_id_names_its_contributing_plugin() {
    // promptforge/web_search migrated to promptforge/web/search: dropping the
    // last segment must yield the contributing capability's id.
    let (tool, _fake) = tool();
    let id = tool.id();
    assert_eq!(id.name(), "search");
    assert_eq!(
        id.plugin(),
        promptforge::plugins::PluginId::parse("promptforge/web").expect("a valid Plugin id")
    );
}

#[tokio::test]
async fn the_provider_receives_the_validated_arguments_as_its_query() {
    let (tool, fake) = tool();
    tool.call(
        TestContext::new().lend(),
        serde_json::json!({
            "query": "hi",
            "count": 5,
            "freshness": "pw",
            "country": "us",
            "search_lang": "en",
            "safesearch": "strict",
            "include_domains": ["example.com"]
        }),
    )
    .await
    .expect("a fully-specified valid request should succeed");

    assert_eq!(
        fake.queries(),
        [SearchQuery {
            query: "hi".to_owned(),
            count: Some(5),
            freshness: Some(Freshness::Pw),
            country: Some("us".to_owned()),
            search_lang: Some("en".to_owned()),
            safesearch: Some(SafeSearch::Strict),
            include_domains: vec!["example.com".to_owned()],
            exclude_domains: Vec::new(),
        }]
    );
}

/// Two results: one with every optional field, one with none.
fn full_and_bare_results() -> SearchResults {
    SearchResults {
        query: "rust".to_owned(),
        results: vec![
            SearchResult {
                title: "Rust".to_owned(),
                url: "https://www.rust-lang.org/".to_owned(),
                description: "A language.".to_owned(),
                age: Some("2 days ago".to_owned()),
                site_name: Some("www.rust-lang.org".to_owned()),
                extra_snippets: vec!["Fast.".to_owned(), "Safe.".to_owned()],
            },
            SearchResult {
                url: "https://e.com".to_owned(),
                ..SearchResult::default()
            },
        ],
    }
}

#[tokio::test]
async fn results_render_as_the_gateways_serialization_and_are_untrusted() {
    let (tool, _fake) = tool_answering(|| Ok(full_and_bare_results()));
    let output = tool
        .call(
            TestContext::new().lend(),
            serde_json::json!({ "query": "rust" }),
        )
        .await
        .expect("the provider's results render");

    assert_eq!(
        output.trust(),
        OutputTrust::Untrusted,
        "external search content must be marked untrusted"
    );
    // The Gateway's `WebSearchResponse` serialization of the same results:
    // its field order, with an absent `age` or `site_name` and empty
    // `extra_snippets` left out.
    assert_eq!(
        output.text(),
        concat!(
            r#"{"query":"rust","results":["#,
            r#"{"title":"Rust","url":"https://www.rust-lang.org/","description":"A language.","#,
            r#""age":"2 days ago","site_name":"www.rust-lang.org","extra_snippets":["Fast.","Safe."]},"#,
            r#"{"title":"","url":"https://e.com","description":""}"#,
            "]}"
        )
    );
}

/// A reply whose second result has a blank URL.
fn blank_url_result() -> SearchResults {
    SearchResults {
        query: "hi".to_owned(),
        results: vec![
            SearchResult {
                url: "https://e.com".to_owned(),
                ..SearchResult::default()
            },
            SearchResult {
                url: "  ".to_owned(),
                ..SearchResult::default()
            },
        ],
    }
}

#[tokio::test]
async fn success_body_with_empty_url_is_rejected() {
    let (tool, _fake) = tool_answering(|| Ok(blank_url_result()));
    let err = tool
        .call(
            TestContext::new().lend(),
            serde_json::json!({ "query": "hi" }),
        )
        .await
        .expect_err("an empty result url must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert_eq!(
        err.to_string(),
        "web_search: malformed search response: result 1 has an empty url"
    );
}

fn transport_failure() -> Result<SearchResults, SearchError> {
    Err(SearchError::with_source(
        SearchErrorKind::Transport,
        "request failed",
        std::io::Error::other("connection refused"),
    ))
}

fn backend_failure() -> Result<SearchResults, SearchError> {
    Err(SearchError::new(
        SearchErrorKind::Backend,
        "backend returned 502: upstream down",
    ))
}

#[tokio::test]
async fn provider_errors_map_their_kind_and_message_and_stay_the_source() {
    let (tool, _fake) = tool_answering(transport_failure);
    let err = tool
        .call(
            TestContext::new().lend(),
            serde_json::json!({ "query": "hi" }),
        )
        .await
        .expect_err("a transport failure fails the call");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert_eq!(err.to_string(), "web_search: request failed");
    let source = std::error::Error::source(&err)
        .and_then(|source| source.downcast_ref::<SearchError>())
        .expect("the provider error is the tool error's source");
    assert_eq!(source.kind(), SearchErrorKind::Transport);
    assert_eq!(
        std::error::Error::source(source).map(ToString::to_string),
        Some("connection refused".to_owned()),
        "the provider's own cause survives the trait boundary"
    );

    let (tool, _fake) = tool_answering(backend_failure);
    let err = tool
        .call(
            TestContext::new().lend(),
            serde_json::json!({ "query": "hi" }),
        )
        .await
        .expect_err("a backend failure fails the call");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert_eq!(
        err.to_string(),
        "web_search: backend returned 502: upstream down"
    );
}

#[tokio::test]
async fn rejects_missing_query() {
    let err = refused(serde_json::json!({ "count": 3 })).await;
    assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
}

#[tokio::test]
async fn rejects_empty_and_oversized_query() {
    assert_eq!(
        refused(serde_json::json!({ "query": "   " })).await.kind(),
        ToolErrorKind::InvalidArguments
    );
    let long = "x".repeat(MAX_QUERY_LEN + 1);
    assert_eq!(
        refused(serde_json::json!({ "query": long })).await.kind(),
        ToolErrorKind::InvalidArguments
    );
}

#[tokio::test]
async fn rejects_unknown_fields_and_bad_optional_types() {
    // Unknown field.
    let err = refused(serde_json::json!({ "query": "hi", "nonsense": 1 })).await;
    assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    assert!(
        std::error::Error::source(&err).is_some(),
        "a deserialization failure must preserve its serde source"
    );
    // Wrong type for count.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": "five" }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    // Out-of-range count.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": MAX_COUNT + 1 }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": 0 }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    // Unknown enum values.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "freshness": "yesterday" }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "safesearch": "maybe" }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
}

#[tokio::test]
async fn rejects_invalid_domain_lists() {
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "include_domains": ["ok.com", "bad/host"] }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    let many: Vec<String> = (0..30).map(|i| format!("h{i}.com")).collect();
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "exclude_domains": many }))
            .await
            .kind(),
        ToolErrorKind::InvalidArguments
    );
}
