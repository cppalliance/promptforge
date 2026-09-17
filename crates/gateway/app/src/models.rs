//! The model catalog surface: `GET /v1/models` and the capability-endpoint
//! status entries the admin status readout renders.

use axum::Json;
use axum::extract::State;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
use crate::model_info;
use crate::wire::ModelInfo;

/// Bearer-authed catalog of configured models for host bind.
pub(crate) async fn list_models(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<model_info::CatalogModelsResponse>, GatewayError> {
    let live = state.live.read().await;
    let data = live
        .routing
        .models()
        .iter()
        .map(|model| {
            model_info::CatalogModelInfo::inference(ModelInfo {
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
        data.extend(
            speech_models
                .iter()
                .map(model_info::CatalogModelInfo::speech),
        );
        data
    };
    Ok(Json(model_info::CatalogModelsResponse {
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
