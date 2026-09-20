//! The `GET /admin/hf/*` routes: a thin bearer-authed proxy onto the
//! Hugging Face hub API, feeding the config UI's Discover view.
//!
//! The proxy forwards the hub's JSON bodies verbatim - the UI adapts the
//! shape - and attaches the boot-time `HF_TOKEN` when one is present, so
//! the browser never holds the token and public repos keep working
//! without one. Upstream 4xx statuses pass through in the gateway's error
//! envelope via [`ProtocolError::upstream_status`]; nothing is cached.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, Method};
use axum::response::Response;
use axum::routing::get;
use gateway_config::Secret;
use gateway_protocol::ProtocolError;
use gateway_protocol::http_util::{self, MAX_ERROR_BODY, read_body_capped};

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, WirePath};
use crate::registry::RouteInfo;

const SEARCH: RouteInfo = RouteInfo::walled("/admin/hf/search", &[Method::GET]);
const MODEL: RouteInfo = RouteInfo::walled("/admin/hf/model/{owner}/{name}", &[Method::GET]);
const README: RouteInfo =
    RouteInfo::walled("/admin/hf/model/{owner}/{name}/readme", &[Method::GET]);

/// The Hugging Face proxy routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[SEARCH, MODEL, README];

/// The Hugging Face proxy routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(SEARCH.path, get(admin_hf_search))
        .route(MODEL.path, get(admin_hf_model))
        .route(README.path, get(admin_hf_readme))
}

/// Whole-request deadline for one hub call, applied per request; reqwest's
/// per-request timeout replaces the bounded client's wider default.
const HF_TIMEOUT: Duration = Duration::from_secs(30);

/// Caps response body at 1 MiB: large model-card READMEs with embedded
/// base64 images can exceed 10 MiB, and the gateway only shows the text.
const MAX_README_BODY: usize = 1024 * 1024;

/// Shared Hugging Face hub client: one reqwest client, the hub base URL,
/// and the boot-time `HF_TOKEN` (absent for anonymous access).
#[derive(Debug)]
pub(crate) struct HfProxy {
    /// The shared bounded HTTP client; the hub deadline is applied per request.
    client: reqwest::Client,
    /// The hub origin, `https://huggingface.co` outside tests.
    base_url: String,
    /// The bearer token sent to the hub, when one was configured.
    token: Option<Secret>,
}

impl HfProxy {
    /// The production hub client: `https://huggingface.co`, with the token
    /// read once from the process `HF_TOKEN` (dotenvy has already folded
    /// the `.env` files into the process env at boot).
    pub(crate) fn from_env() -> HfProxy {
        let token = std::env::var("HF_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .map(Secret::new);
        HfProxy::new("https://huggingface.co".to_owned(), token)
    }

    /// A hub client aimed at `base_url` with an explicit token, so tests
    /// point the proxy at a local stub without touching the process env.
    pub(crate) fn new(base_url: String, token: Option<Secret>) -> HfProxy {
        HfProxy {
            client: http_util::bounded_client(),
            base_url,
            token,
        }
    }

    /// GETs `{base_url}{path}` with `query`, forwarding the hub's JSON body
    /// and status verbatim on success and mapping a non-success status or a
    /// transport failure into the gateway's error envelope.
    async fn forward(&self, path: &str, query: &[(&str, &str)]) -> Result<Response, GatewayError> {
        let mut request = self
            .client
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .timeout(HF_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose());
        }
        let response = request
            .send()
            .await
            .map_err(ProtocolError::upstream_transport)?;
        let status = response.status();
        if !status.is_success() {
            let body = read_body_capped(response, MAX_ERROR_BODY).await;
            let body: String = body.chars().take(2000).collect();
            return Err(ProtocolError::upstream_status(status.as_u16(), body).into());
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or(HeaderValue::from_static("application/json"));
        // Streaming the body through keeps the gateway's memory use flat no
        // matter how large the hub's sibling list gets.
        Response::builder()
            .status(status)
            .header(CONTENT_TYPE, content_type)
            .body(Body::from_stream(response.bytes_stream()))
            .map_err(GatewayError::upstream_protocol)
    }

    /// GETs `{base_url}{path}`, returning the body as `text/markdown` on
    /// success, a plain 404 for a missing README, and the error envelope
    /// for other failures. The body is capped at [`MAX_README_BODY`].
    async fn forward_readme(&self, path: &str) -> Result<Response, GatewayError> {
        let mut request = self
            .client
            .get(format!("{}{path}", self.base_url))
            .timeout(HF_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose());
        }
        let response = request
            .send()
            .await
            .map_err(ProtocolError::upstream_transport)?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Response::builder()
                .status(404)
                .body(Body::empty())
                .map_err(GatewayError::upstream_protocol);
        }
        if !status.is_success() {
            let body = read_body_capped(response, MAX_ERROR_BODY).await;
            let body: String = body.chars().take(2000).collect();
            return Err(ProtocolError::upstream_status(status.as_u16(), body).into());
        }
        let body = read_body_capped(response, MAX_README_BODY).await;
        Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "text/markdown; charset=utf-8")
            .body(Body::from(body))
            .map_err(GatewayError::upstream_protocol)
    }
}

/// Query parameters accepted by `GET /admin/hf/search`; each present field
/// is forwarded to the hub's model-search API, and everything else is
/// dropped at this boundary.
#[derive(Debug, Default)]
pub(crate) struct HfSearchQuery {
    /// Free-text search, forwarded as the hub's `search` parameter.
    q: Option<String>,
    /// Tag filter; the Discover view pins `gguf`.
    filter: Option<String>,
    /// Sort field: `downloads`, `trendingScore`, or `lastModified`.
    sort: Option<String>,
    /// Sort direction, `-1` for descending.
    direction: Option<String>,
    /// Result page size.
    limit: Option<String>,
    /// `full=true` asks the hub to include each result's sibling file list.
    full: Option<String>,
    /// One workload tag. The UI fans out requests to implement OR filters.
    pipeline_tag: Option<String>,
}

/// The `GET /admin/hf/search` route: bearer-authed, proxies the hub's
/// `GET /api/models` search and returns its JSON body verbatim.
pub(crate) async fn admin_hf_search(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    _caller: LoopbackCaller,
) -> Result<Response, GatewayError> {
    let query = parse_search_query(query.as_deref())?;
    let renames = [("search", &query.q)];
    let passthrough = [
        ("filter", &query.filter),
        ("sort", &query.sort),
        ("direction", &query.direction),
        ("limit", &query.limit),
        ("full", &query.full),
    ];
    let mut params: Vec<(&str, &str)> = renames
        .iter()
        .chain(passthrough.iter())
        .filter_map(|(name, value)| Some((*name, value.as_deref()?)))
        .collect();
    if let Some(tag) = query.pipeline_tag.as_deref() {
        params.push(("pipeline_tag", tag));
    }
    state.hf.forward("/api/models", &params).await
}

/// Parses the small search query allowlist. Every field is singular and
/// closed-set values are validated before any upstream request.
fn parse_search_query(raw: Option<&str>) -> Result<HfSearchQuery, GatewayError> {
    let mut query = HfSearchQuery::default();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let slot = match key.as_ref() {
            "q" => &mut query.q,
            "filter" => &mut query.filter,
            "sort" => &mut query.sort,
            "direction" => &mut query.direction,
            "limit" => &mut query.limit,
            "full" => &mut query.full,
            "pipeline_tag" => {
                if !matches!(
                    value.as_ref(),
                    "text-generation"
                        | "feature-extraction"
                        | "sentence-similarity"
                        | "text-classification"
                        | "automatic-speech-recognition"
                        | "text-to-image"
                        | "text-to-speech"
                ) {
                    return Err(GatewayError::MalformedRequest(format!(
                        "unsupported pipeline_tag {value:?}"
                    )));
                }
                if query.pipeline_tag.replace(value.into_owned()).is_some() {
                    return Err(GatewayError::MalformedRequest(
                        "pipeline_tag must appear at most once".to_owned(),
                    ));
                }
                continue;
            }
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(GatewayError::MalformedRequest(format!(
                "duplicate query field {key}"
            )));
        }
    }
    validate_search_value("filter", query.filter.as_deref(), &["gguf"])?;
    validate_search_value(
        "sort",
        query.sort.as_deref(),
        &["downloads", "trendingScore", "lastModified"],
    )?;
    validate_search_value("direction", query.direction.as_deref(), &["-1"])?;
    validate_search_value("full", query.full.as_deref(), &["true"])?;
    if let Some(limit) = query.limit.as_deref()
        && !limit
            .parse::<u16>()
            .is_ok_and(|parsed| (1..=100).contains(&parsed))
    {
        return Err(GatewayError::MalformedRequest(
            "limit must be an integer from 1 through 100".to_owned(),
        ));
    }
    Ok(query)
}

/// Validates one optional search field against its closed value set.
fn validate_search_value(
    name: &str,
    value: Option<&str>,
    accepted: &[&str],
) -> Result<(), GatewayError> {
    if let Some(value) = value
        && !accepted.contains(&value)
    {
        return Err(GatewayError::MalformedRequest(format!(
            "unsupported {name} value {value:?}"
        )));
    }
    Ok(())
}

/// The `GET /admin/hf/model/{owner}/{name}` route: bearer-authed, proxies
/// the hub's model detail for an `owner/name` repo with `blobs=true`, so
/// the sibling list carries the exact file sizes the quant picker needs.
pub(crate) async fn admin_hf_model(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
    WirePath((owner, name)): WirePath<(String, String)>,
) -> Result<Response, GatewayError> {
    let repo = format!("{owner}/{name}");
    validate_repo(&repo)?;
    state
        .hf
        .forward(&format!("/api/models/{repo}"), &[("blobs", "true")])
        .await
}

/// The `GET /admin/hf/model/{owner}/{name}/readme` route: bearer-authed,
/// proxies the hub's raw README.md for the repo and returns it as
/// `text/markdown; charset=utf-8`. A missing README maps to 404.
pub(crate) async fn admin_hf_readme(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
    WirePath((owner, name)): WirePath<(String, String)>,
) -> Result<Response, GatewayError> {
    let repo = format!("{owner}/{name}");
    validate_repo(&repo)?;
    state
        .hf
        .forward_readme(&format!("/{repo}/raw/main/README.md"))
        .await
}

/// Checks that `repo` is exactly `owner/name`: two non-empty segments of
/// hub-legal characters (ASCII alphanumerics, `-`, `_`, `.`), neither made
/// only of dots.
fn validate_repo(repo: &str) -> Result<(), GatewayError> {
    let mut segments = repo.split('/');
    if let (Some(owner), Some(name), None) = (segments.next(), segments.next(), segments.next())
        && is_repo_segment(owner)
        && is_repo_segment(name)
    {
        return Ok(());
    }
    Err(GatewayError::MalformedRequest(format!(
        "repo `{repo}` is not an owner/name pair of path-safe segments"
    )))
}

/// Whether one repo segment is non-empty, hub-legal, and not a dot run
/// (`.` and `..` are path traversal, not names).
fn is_repo_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.bytes().all(|byte| byte == b'.')
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
#[path = "hf-tests.rs"]
mod tests;
