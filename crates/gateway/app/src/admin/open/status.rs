//! The `GET /admin/status` readout: profile, models, queue, and one
//! readiness entry per capability endpoint.

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use gateway_api_types::Progress;
use gateway_config::ModelKind;
use serde::Serialize;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
#[cfg(feature = "stt")]
use crate::models::with_speech_endpoint;
use crate::models::{EndpointStatus, endpoint_status};
use crate::registry::RouteInfo;
#[cfg(feature = "stt")]
use crate::speech::SpeechSnapshot;

const STATUS: RouteInfo = RouteInfo::open("/admin/status", &[Method::GET]);

/// The status route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[STATUS];

/// The status route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(STATUS.path, get(admin_status))
}

/// The `GET /admin/status` reply.
#[derive(Debug, Serialize)]
pub(crate) struct StatusReply {
    /// The running profile's name, `null` when none is selected.
    profile: Option<String>,
    /// Every model in the live routing table, in catalog order.
    models: Vec<String>,
    /// The boot profile's local models whose children are still spawning.
    loading_models: Vec<String>,
    /// The process-lifetime identifier the config UI uses to detect a
    /// restart.
    config_generation: String,
    /// The active profile's `models` allowlist, when it declared one.
    model_allowlist: Option<Vec<String>>,
    /// Running local children; zero in a headless build.
    local_children: usize,
    /// The declared VRAM total of the running local and speech models.
    vram_gb: f64,
    /// The hub's current busy flag and text.
    progress: Progress,
    /// The command queue's active and waiting commands.
    queue: QueueReply,
    /// One readiness entry per capability endpoint.
    endpoints: Vec<EndpointStatus>,
    /// Speech lifecycle facts (`stt` builds).
    #[cfg(feature = "stt")]
    speech: SpeechSnapshot,
}

/// The command queue as the status readout reports it.
#[derive(Debug, Serialize)]
pub(crate) struct QueueReply {
    /// The command the worker is running, if any.
    active: Option<ActiveCommandReply>,
    /// The commands waiting behind it, in queue order.
    pending: Vec<PendingCommandReply>,
}

/// The active command: its display name and when it started, as Unix
/// epoch seconds.
#[derive(Debug, Serialize)]
pub(crate) struct ActiveCommandReply {
    name: String,
    started_at: u64,
}

/// One waiting command: its display name and when it was queued, as Unix
/// epoch seconds.
#[derive(Debug, Serialize)]
pub(crate) struct PendingCommandReply {
    name: String,
    queued_at: u64,
}

/// An `Instant` as Unix epoch seconds for the status wire shape. The
/// conversion goes through the elapsed duration, so a clock that jumped
/// backward clamps to now rather than underflowing.
fn instant_epoch_seconds(instant: std::time::Instant) -> u64 {
    let elapsed = instant.elapsed();
    std::time::SystemTime::now()
        .checked_sub(elapsed)
        .unwrap_or_else(std::time::SystemTime::now)
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Current profile name, loaded model names, the local models the boot
/// load is still spawning, process config generation, the profile's model
/// allowlist (its local and speech-to-text members), the declared VRAM
/// total, the hub's current [`Progress`](gateway_api_types::Progress)
/// snapshot, the command queue's active and pending commands, and one
/// readiness entry per capability endpoint the gateway can serve.
pub(crate) async fn admin_status(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<StatusReply>, GatewayError> {
    let active = state.commands.active_command();
    let pending = state.commands.pending_commands();
    let progress = state.hub.current();
    let live = state.live.read().await;
    let models: Vec<String> = live
        .routing
        .models()
        .iter()
        .map(|m| m.name.clone())
        .collect();
    // A headless build has no local runtime; it reports zero children rather
    // than dropping the field from the status response.
    #[cfg(feature = "local")]
    let local_children = live.local.child_count();
    #[cfg(not(feature = "local"))]
    let local_children = 0;
    let (_, vram_gb) = live.model_status();
    let command_active = active.is_some();
    let configured = |kind: ModelKind| {
        live.config
            .models()
            .iter()
            .any(|model| model.kind() == kind)
            || live
                .config
                .catalog_local_models()
                .iter()
                .any(|model| model.kind() == kind)
    };
    let routed = |kind: ModelKind| live.routing.models().iter().any(|model| model.kind == kind);
    let endpoints = [
        ("/v1/chat/completions", "Chat completions", ModelKind::Chat),
        ("/v1/embeddings", "Embeddings", ModelKind::Embedding),
        ("/v1/rerank", "Rerank", ModelKind::Classifier),
        ("/v1/audio/speech", "Speech synthesis", ModelKind::Speech),
    ]
    .into_iter()
    .map(|(path, name, kind)| {
        endpoint_status(path, name, configured(kind), routed(kind), command_active)
    })
    .collect::<Vec<_>>();
    #[cfg(feature = "stt")]
    let (endpoints, speech) =
        with_speech_endpoint(endpoints, state.speech.status(), command_active);
    Ok(Json(StatusReply {
        profile: live.profile_name.clone(),
        models,
        loading_models: live.loading.iter().cloned().collect(),
        config_generation: state.config_generation.to_string(),
        model_allowlist: live.model_allowlist.clone(),
        local_children,
        vram_gb,
        progress,
        queue: QueueReply {
            active: active.map(|status| ActiveCommandReply {
                name: status.name,
                started_at: instant_epoch_seconds(status.started_at),
            }),
            pending: pending
                .into_iter()
                .map(|entry| PendingCommandReply {
                    name: entry.name,
                    queued_at: instant_epoch_seconds(entry.queued_at),
                })
                .collect(),
        },
        endpoints,
        #[cfg(feature = "stt")]
        speech: SpeechSnapshot::from(speech),
    }))
}

#[cfg(test)]
#[path = "status-tests.rs"]
mod tests;
