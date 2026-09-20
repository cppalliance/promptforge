//! The `POST /v1/tools/web_search` tool route: bearer-authed, delegates
//! to the web-search service crate. Compiled only with the `web-search`
//! feature, since the route has nothing to delegate to without it.

use axum::extract::State;
use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use gateway_web_search::{WebSearchRequest, WebSearchResponse};

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, WireJson};
use crate::registry::RouteInfo;

const WEB_SEARCH: RouteInfo = RouteInfo::open("/v1/tools/web_search", &[Method::POST]);

/// The web-search tool route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[WEB_SEARCH];

/// The web-search tool route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(WEB_SEARCH.path, post(web_search))
}

/// The `POST /v1/tools/web_search` route.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, [`GatewayError::MalformedRequest`] when the request
/// fails validation, and the upstream variants on a provider failure.
async fn web_search(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    WireJson(request): WireJson<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    let service = state
        .web_search()
        .await
        .ok_or(GatewayError::ToolNotConfigured("web_search"))?;
    Ok(Json(service.search(&request).await?))
}
