//! The model catalog surface: `GET /v1/models`, its wire shape, and the
//! capability-endpoint status entries the admin status readout renders.

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
use crate::registry::RouteInfo;
use crate::wire::ModelInfo;

const LIST_MODELS: RouteInfo = RouteInfo::open("/v1/models", &[Method::GET]);

/// The catalog route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[LIST_MODELS];

/// The catalog route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(LIST_MODELS.path, get(list_models))
}

/// The model-list wire response, including routed and active speech models.
#[derive(Debug, Serialize)]
pub(crate) struct CatalogModelsResponse {
    /// Always `"list"`.
    pub(crate) object: &'static str,
    /// Models currently accepting their respective request shape.
    pub(crate) data: Vec<CatalogModelInfo>,
}

/// One routed inference model or active speech model.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum CatalogModelInfo {
    /// Existing chat, embedding, classifier, or speech metadata.
    Inference(ModelInfo),
    /// Generic transcription metadata.
    #[cfg(feature = "stt")]
    Speech(SpeechCatalogModelInfo),
}

impl CatalogModelInfo {
    pub(crate) fn inference(model: ModelInfo) -> Self {
        Self::Inference(model)
    }

    #[cfg(feature = "stt")]
    pub(crate) fn speech(model: &gateway_stt::SpeechModelInfo) -> Self {
        Self::Speech(SpeechCatalogModelInfo {
            id: model.name().to_owned(),
            object: "model",
            kind: "transcription",
        })
    }
}

/// Speech metadata contains only fields meaningful to transcription clients.
#[cfg(feature = "stt")]
#[derive(Debug, Serialize)]
pub(crate) struct SpeechCatalogModelInfo {
    id: String,
    object: &'static str,
    kind: &'static str,
}

/// Bearer-authed catalog of configured models for host bind.
pub(crate) async fn list_models(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<CatalogModelsResponse>, GatewayError> {
    let live = state.live.read().await;
    let data = live
        .routing
        .models()
        .iter()
        .map(|model| {
            CatalogModelInfo::inference(ModelInfo {
                id: model.name.clone(),
                object: "model",
                kind: model.kind,
                description: model.description.clone(),
                context: model.context,
                thinking: model.thinking,
                capabilities: model.capabilities.clone(),
            })
        })
        .collect::<Vec<_>>();
    drop(live);
    #[cfg(feature = "stt")]
    let data = {
        let mut data = data;
        let speech_models = state.speech.models();
        data.extend(speech_models.iter().map(CatalogModelInfo::speech));
        data
    };
    Ok(Json(CatalogModelsResponse {
        object: "list",
        data,
    }))
}

/// One capability endpoint's readout in the `GET /admin/status` response:
/// the route path, a display name, whether the live routing table serves
/// it, and whether a queue command is provisioning its models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndpointStatus {
    pub(crate) path: &'static str,
    pub(crate) name: &'static str,
    pub(crate) ready: bool,
    pub(crate) provisioning: bool,
}

/// Maps one capability endpoint's facts to its status entry. `ready` is
/// the routing table's live state; `provisioning` means the configuration
/// selects a model for the endpoint, none is loaded yet, and a queue
/// command is running. A configured endpoint with no command running reads
/// as simply not ready - the config UI's LED strip renders both
/// not-ready states gray and reserves amber for active provisioning.
pub(crate) fn endpoint_status(
    path: &'static str,
    name: &'static str,
    configured: bool,
    ready: bool,
    command_active: bool,
) -> EndpointStatus {
    EndpointStatus {
        path,
        name,
        ready,
        provisioning: configured && !ready && command_active,
    }
}

#[cfg(feature = "stt")]
pub(crate) fn with_speech_endpoint(
    mut endpoints: Vec<EndpointStatus>,
    speech: gateway_stt::SpeechStatus,
    command_active: bool,
) -> (Vec<EndpointStatus>, gateway_stt::SpeechStatus) {
    endpoints.push(endpoint_status(
        "/v1/audio/transcriptions",
        "Audio transcriptions",
        speech.configured(),
        speech.ready(),
        command_active,
    ));
    (endpoints, speech)
}

#[cfg(all(test, feature = "stt"))]
#[path = "models-tests.rs"]
mod tests;
