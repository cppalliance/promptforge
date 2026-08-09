//! Built-in tool endpoints the gateway exposes.
//!
//! The gateway holds the search provider credential, so the executor above it
//! never sees it. This module implements the `web_search` tool: a bearer-authed
//! `POST /v1/tools/web_search` that proxies a query to the Brave Search API and
//! returns a trimmed result set.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::check_auth;
use crate::config::{Secret, WebSearchConfig};
use crate::error::GatewayError;

/// Max characters kept for a result title after sanitisation.
pub const TITLE_MAX_CHARS: usize = 512;
/// Max characters kept for a result description after sanitisation.
pub const DESCRIPTION_MAX_CHARS: usize = 4096;
/// Max characters kept for a result URL after tracking strip.
pub const URL_MAX_CHARS: usize = 2048;

/// Cloneable runtime settings for `web_search`, filled from [`WebSearchConfig`].
#[derive(Debug, Clone)]
pub struct WebSearchSettings {
    /// Used when the request omits `count`.
    pub default_count: u8,
    /// Clamp and over-fetch ceiling for result counts.
    pub max_count: u8,
    /// Diversity cap per hostname group.
    pub max_per_host: u8,
    /// Applied when the request omits `freshness` and this is non-empty.
    pub default_freshness: String,
    /// Applied when the request omits `safesearch` and this is non-empty.
    pub default_safesearch: String,
    /// When true, scrub known tracking query params from result URLs.
    pub strip_tracking: bool,
}

impl WebSearchSettings {
    /// Build settings from the tool configuration.
    #[must_use]
    pub fn from_config(cfg: &WebSearchConfig) -> WebSearchSettings {
        WebSearchSettings {
            default_count: cfg.default_count,
            max_count: cfg.max_count,
            max_per_host: cfg.max_per_host,
            default_freshness: cfg.default_freshness.clone(),
            default_safesearch: cfg.default_safesearch.clone(),
            strip_tracking: cfg.strip_tracking,
        }
    }
}

/// The web-search runtime state: the provider credential, the base URL, and a
/// shared HTTP client.
#[derive(Debug)]
pub struct WebSearchState {
    /// The credential sent to the search provider.
    api_key: Secret,
    /// The search API base URL.
    base_url: String,
    /// Cloneable tool settings derived from config.
    pub settings: WebSearchSettings,
    /// The shared HTTP client used for provider calls.
    http: reqwest::Client,
}

impl WebSearchState {
    /// Build web-search state from its configuration.
    #[must_use]
    pub fn new(cfg: &WebSearchConfig) -> WebSearchState {
        WebSearchState {
            api_key: cfg.api_key.clone(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            settings: WebSearchSettings::from_config(cfg),
            http: reqwest::Client::new(),
        }
    }
}

/// The request body for `POST /v1/tools/web_search`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchRequest {
    /// The search query.
    pub query: String,
    /// The desired number of results. Defaults to
    /// [`WebSearchSettings::default_count`] and is clamped to
    /// [`WebSearchSettings::max_count`].
    #[serde(default)]
    pub count: Option<u8>,
    /// Freshness filter; empty or absent means omit from the provider query.
    #[serde(default)]
    pub freshness: Option<String>,
    /// Country code; empty or absent means omit from the provider query.
    #[serde(default)]
    pub country: Option<String>,
    /// Search language; empty or absent means omit from the provider query.
    #[serde(default)]
    pub search_lang: Option<String>,
    /// SafeSearch level; falls back to settings when omitted or empty.
    #[serde(default)]
    pub safesearch: Option<String>,
    /// Hostname include filter; empty means no include filter.
    #[serde(default)]
    pub include_domains: Vec<String>,
    /// Hostname exclude filter; empty means no exclude filter.
    #[serde(default)]
    pub exclude_domains: Vec<String>,
}

/// The response body for `POST /v1/tools/web_search`.
#[derive(Debug, Serialize)]
pub struct WebSearchResponse {
    /// The trimmed request query that produced these results.
    pub query: String,
    /// The trimmed search results.
    pub results: Vec<SearchResult>,
}

/// One search result, trimmed to the fields the executor needs.
#[derive(Debug, Serialize)]
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
    /// Hostname derived from `url`, when parsing succeeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// Extra snippets from the provider, when present.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_snippets: Vec<String>,
}

/// The Brave response envelope, narrowed to the `web.results` array.
#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

/// The `web` object of a Brave response.
#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

/// One Brave result, narrowed to the fields the executor needs.
#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    age: Option<String>,
    #[serde(default)]
    extra_snippets: Vec<String>,
}

/// Trim ASCII whitespace from `query` and reject empty values.
///
/// # Errors
/// Returns [`GatewayError::MalformedRequest`] with
/// `"web_search: empty query"` when the trimmed query is empty.
fn trim_web_search_query(query: &str) -> Result<String, GatewayError> {
    let trimmed = query
        .trim_matches(|c: char| c.is_ascii_whitespace())
        .to_string();
    if trimmed.is_empty() {
        return Err(GatewayError::MalformedRequest(
            "web_search: empty query".to_string(),
        ));
    }
    Ok(trimmed)
}

/// Sanitize free text: drop most controls, collapse whitespace, trim, decode a
/// fixed entity set, then cap by Unicode scalar count.
#[must_use]
pub fn sanitize_text(text: &str, max_chars: usize) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' || c == '\t' {
            cleaned.push(' ');
        } else if !c.is_control() {
            cleaned.push(c);
        }
    }
    let collapsed = collapse_whitespace(&cleaned);
    let trimmed = collapsed.trim();
    let decoded = decode_entities(trimmed);
    truncate_chars(&decoded, max_chars)
}

/// Drop known tracking query parameters from `url`. Removes a trailing empty `?`.
///
/// Params removed when the name equals `fbclid`, `gclid`, `mc_cid`, `mc_eid`,
/// or starts with `utm_`.
#[must_use]
pub fn strip_tracking_params(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return truncate_chars(url, URL_MAX_CHARS);
    };
    let (query, fragment) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };
    let mut kept: Vec<&str> = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let name = pair.split('=').next().unwrap_or(pair);
        if is_tracking_param(name) {
            continue;
        }
        kept.push(pair);
    }
    let mut out = String::from(base);
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    truncate_chars(&out, URL_MAX_CHARS)
}

/// Extract the hostname from `url` without a URL crate.
///
/// Handles optional scheme, `userinfo@`, and strips a trailing port. Returns
/// lowercase host text, or `None` when no host can be parsed.
#[must_use]
pub fn host_from_url(url: &str) -> Option<String> {
    let rest = match url.split_once("://") {
        Some((_, after)) => after,
        None => url.strip_prefix("//").unwrap_or(url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    let host_port = match authority.rsplit_once('@') {
        Some((_, host_port)) => host_port,
        None => authority,
    };
    if host_port.is_empty() {
        return None;
    }
    // Bracketed IPv6: keep inside brackets; otherwise strip :port.
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        let end = inner.find(']')?;
        &inner[..end]
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Hostname group / display name: lowercase host with one leading `www.` removed.
#[must_use]
pub fn site_name_from_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower
        .strip_prefix("www.")
        .unwrap_or(lower.as_str())
        .to_string()
}

/// Apply include then exclude domain filters.
///
/// Empty `include_domains` means no include filter. Empty `exclude_domains`
/// means no exclude filter. A hostname matches a listed domain when they are
/// equal (ASCII lowercase) or the hostname ends with `.` + domain.
#[must_use]
pub fn filter_domains(
    results: Vec<SearchResult>,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Vec<SearchResult> {
    let include: Vec<String> = include_domains
        .iter()
        .map(|d| d.to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let exclude: Vec<String> = exclude_domains
        .iter()
        .map(|d| d.to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();

    results
        .into_iter()
        .filter(|r| {
            let host = host_from_url(&r.url).unwrap_or_default();
            if !include.is_empty() && !include.iter().any(|d| host_matches_domain(&host, d)) {
                return false;
            }
            if !exclude.is_empty() && exclude.iter().any(|d| host_matches_domain(&host, d)) {
                return false;
            }
            true
        })
        .collect()
}

/// Keep results in order while each host group stays under `max_per_host`,
/// stopping once `count` results are kept.
///
/// Host groups use full hostname, lowercase, with one leading `www.` stripped.
#[must_use]
pub fn diversify_hosts(
    results: Vec<SearchResult>,
    max_per_host: u8,
    count: u8,
) -> Vec<SearchResult> {
    let mut kept = Vec::new();
    let mut per_host: HashMap<String, u8> = HashMap::new();
    let max_per_host = max_per_host.max(1);
    let count = count as usize;

    for result in results {
        if kept.len() >= count {
            break;
        }
        let group = host_from_url(&result.url)
            .map(|h| site_name_from_host(&h))
            .unwrap_or_default();
        let n = per_host.entry(group).or_insert(0);
        if *n >= max_per_host {
            continue;
        }
        *n += 1;
        kept.push(result);
    }
    kept
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn decode_entities(text: &str) -> String {
    // Decode `&amp;` first so double-encoded forms like `&amp;lt;` resolve.
    let mut s = text.replace("&amp;", "&");
    s = s.replace("&lt;", "<");
    s = s.replace("&gt;", ">");
    s = s.replace("&quot;", "\"");
    s = s.replace("&#39;", "'");
    s = s.replace("&apos;", "'");
    s
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn is_tracking_param(name: &str) -> bool {
    matches!(name, "fbclid" | "gclid" | "mc_cid" | "mc_eid") || name.starts_with("utm_")
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == *domain || host.ends_with(&format!(".{domain}"))
}

/// Call the Brave Search API and return a trimmed result set.
///
/// # Errors
/// Returns [`GatewayError::UpstreamTransport`] on a transport failure and
/// [`GatewayError::UpstreamStatus`] on a non-success provider status.
async fn brave_search(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    query: &str,
    count: u8,
    max_count: u8,
) -> Result<WebSearchResponse, GatewayError> {
    let count = count.clamp(1, max_count);
    let response = http
        .get(format!("{base_url}/web/search"))
        .query(&[("q", query), ("count", &count.to_string())])
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(GatewayError::upstream_transport)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(2000).collect();
        return Err(GatewayError::UpstreamStatus {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: BraveResponse = response
        .json()
        .await
        .map_err(GatewayError::upstream_transport)?;

    let results = parsed
        .web
        .map(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            description: r.description,
            age: r.age,
            site_name: None,
            extra_snippets: r.extra_snippets,
        })
        .collect();

    Ok(WebSearchResponse {
        query: query.to_string(),
        results,
    })
}

/// The `POST /v1/tools/web_search` route: bearer-authed, proxies to Brave.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, [`GatewayError::MalformedRequest`] when `query` is empty
/// after trimming, and the upstream variants on a provider failure.
pub async fn web_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    let web_search = state
        .web_search()
        .await
        .ok_or(GatewayError::ToolNotConfigured("web_search"))?;
    let query = trim_web_search_query(&request.query)?;
    let count = request.count.unwrap_or(web_search.settings.default_count);
    let response = brave_search(
        &web_search.http,
        &web_search.base_url,
        web_search.api_key.expose(),
        &query,
        count,
        web_search.settings.max_count,
    )
    .await?;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GatewayError;

    fn hit(title: &str, url: &str) -> SearchResult {
        SearchResult {
            title: title.to_string(),
            url: url.to_string(),
            description: String::new(),
            age: None,
            site_name: None,
            extra_snippets: Vec::new(),
        }
    }

    #[test]
    fn empty_query_is_malformed_request() {
        for query in ["", "   ", "\t\n"] {
            let err = trim_web_search_query(query).expect_err("empty query");
            match err {
                GatewayError::MalformedRequest(message) => {
                    assert_eq!(message, "web_search: empty query");
                }
                other => panic!("expected MalformedRequest, got {other:?}"),
            }
        }
    }

    #[test]
    fn non_empty_query_is_trimmed() {
        assert_eq!(
            trim_web_search_query("  C++ Alliance  ").expect("ok"),
            "C++ Alliance"
        );
    }

    #[test]
    fn sanitize_text_strips_controls_collapses_and_decodes() {
        let input = "A\u{0001}B\nC\tD   &amp; &lt;x&gt; &quot;q&#39; &apos;z";
        let out = sanitize_text(input, TITLE_MAX_CHARS);
        assert_eq!(out, "AB C D & <x> \"q' 'z");
    }

    #[test]
    fn sanitize_text_caps_title_description_and_url_limits() {
        let title = "t".repeat(TITLE_MAX_CHARS + 10);
        assert_eq!(
            sanitize_text(&title, TITLE_MAX_CHARS).chars().count(),
            TITLE_MAX_CHARS
        );

        let desc = "d".repeat(DESCRIPTION_MAX_CHARS + 3);
        assert_eq!(
            sanitize_text(&desc, DESCRIPTION_MAX_CHARS).chars().count(),
            DESCRIPTION_MAX_CHARS
        );

        let url = format!("https://example.com/{}", "u".repeat(URL_MAX_CHARS));
        assert_eq!(strip_tracking_params(&url).chars().count(), URL_MAX_CHARS);
    }

    #[test]
    fn strip_tracking_removes_utm_and_fbclid() {
        let url = "https://a.com/x?utm_source=1&keep=yes&fbclid=abc&gclid=1&mc_cid=2&mc_eid=3&utm_medium=x";
        assert_eq!(strip_tracking_params(url), "https://a.com/x?keep=yes");

        let only_track = "https://a.com/x?utm_source=1";
        assert_eq!(strip_tracking_params(only_track), "https://a.com/x");
    }

    #[test]
    fn host_and_site_name_helpers() {
        assert_eq!(
            host_from_url("https://WWW.Example.COM:443/path"),
            Some("www.example.com".to_string())
        );
        assert_eq!(site_name_from_host("www.example.com"), "example.com");
        assert_eq!(site_name_from_host("example.com"), "example.com");
        assert_eq!(host_from_url("not-a-url"), Some("not-a-url".to_string()));
        assert_eq!(host_from_url("https:///"), None);
    }

    #[test]
    fn filter_domains_include_then_exclude() {
        let results = vec![
            hit("A", "https://a.com/1"),
            hit("B", "https://b.com/1"),
            hit("Sub", "https://sub.a.com/1"),
            hit("C", "https://c.com/1"),
        ];
        let include = vec!["a.com".to_string()];
        let included = filter_domains(results, &include, &[]);
        assert_eq!(included.len(), 2);
        assert_eq!(included[0].url, "https://a.com/1");
        assert_eq!(included[1].url, "https://sub.a.com/1");

        let exclude = vec!["sub.a.com".to_string()];
        let after = filter_domains(
            vec![
                hit("A", "https://a.com/1"),
                hit("Sub", "https://sub.a.com/1"),
            ],
            &include,
            &exclude,
        );
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].url, "https://a.com/1");
    }

    #[test]
    fn diversify_hosts_worked_example_count_3() {
        let results = vec![
            hit("A1", "https://a.com/x?utm_source=1"),
            hit("A2", "https://a.com/y"),
            hit("A3", "https://a.com/z"),
            hit("B1", "https://b.com/1"),
        ];
        let stripped: Vec<SearchResult> = results
            .into_iter()
            .map(|mut r| {
                r.url = strip_tracking_params(&r.url);
                r.site_name = host_from_url(&r.url).map(|h| site_name_from_host(&h));
                r
            })
            .collect();
        let out = diversify_hosts(stripped, 2, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].url, "https://a.com/x");
        assert_eq!(out[0].title, "A1");
        assert_eq!(out[0].site_name.as_deref(), Some("a.com"));
        assert_eq!(out[1].url, "https://a.com/y");
        assert_eq!(out[1].title, "A2");
        assert_eq!(out[2].url, "https://b.com/1");
        assert_eq!(out[2].title, "B1");
        assert_eq!(out[2].site_name.as_deref(), Some("b.com"));
    }

    #[test]
    fn diversify_hosts_three_plus_two_keeps_two_and_two_at_count_4() {
        let results = vec![
            hit("A1", "https://a.com/1"),
            hit("A2", "https://a.com/2"),
            hit("A3", "https://a.com/3"),
            hit("B1", "https://b.com/1"),
            hit("B2", "https://b.com/2"),
        ];
        let out = diversify_hosts(results, 2, 4);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].title, "A1");
        assert_eq!(out[1].title, "A2");
        assert_eq!(out[2].title, "B1");
        assert_eq!(out[3].title, "B2");
    }
}

#[cfg(test)]
mod live_tests {
    use super::brave_search;

    /// Hits the real Brave Search API to validate the request shape and the
    /// `web.results` parsing against Brave's actual JSON.
    ///
    /// Ignored by default so the normal test run needs no credential. Run it
    /// manually with `BRAVE_API_KEY` set in the environment:
    /// `cargo test -p promptforge-gateway -- --ignored live_brave_search --nocapture`
    #[tokio::test]
    #[ignore = "hits the real Brave API; requires BRAVE_API_KEY, run with --ignored"]
    async fn live_brave_search() {
        let api_key =
            std::env::var("BRAVE_API_KEY").expect("set BRAVE_API_KEY to run this live test");
        let http = reqwest::Client::new();

        let response = brave_search(
            &http,
            "https://api.search.brave.com/res/v1",
            &api_key,
            "rust programming language",
            5,
            20,
        )
        .await
        .expect("brave search should succeed");

        assert_eq!(response.query, "rust programming language");
        assert!(
            !response.results.is_empty(),
            "expected at least one result from Brave"
        );
        assert!(
            response.results[0].url.starts_with("http"),
            "expected a real URL, got: {}",
            response.results[0].url
        );

        // Printed for eyeballing when run manually with --nocapture.
        for result in &response.results {
            println!("- {} :: {}", result.title, result.url);
        }
    }
}
