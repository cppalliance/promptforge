//! The `GET /admin/status` readout: profile, models, queue, and one
//! readiness entry per capability endpoint.

use axum::Json;
use axum::extract::State;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
use crate::models::endpoint_status;
#[cfg(feature = "stt")]
use crate::models::with_speech_endpoint;
use gateway_config::ModelKind;

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
) -> Result<Json<serde_json::Value>, GatewayError> {
    let active = state.commands.active_command();
    let pending = state.commands.pending_commands();
    let progress = state.hub.current();
    let live = state.live.read().await;
    let models: Vec<&str> = live
        .routing
        .models()
        .iter()
        .map(|m| m.name.as_str())
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
    let response = serde_json::json!({
        "profile": live.profile_name,
        "models": models,
        "loading_models": live.loading.iter().collect::<Vec<_>>(),
        "config_generation": state.config_generation.as_ref(),
        "model_allowlist": live.model_allowlist,
        "local_children": local_children,
        "vram_gb": vram_gb,
        "progress": progress,
        "queue": {
            "active": active.map(|status| serde_json::json!({
                "name": status.name,
                "started_at": instant_epoch_seconds(status.started_at),
            })),
            "pending": pending
                .iter()
                .map(|entry| serde_json::json!({
                    "name": entry.name,
                    "queued_at": instant_epoch_seconds(entry.queued_at),
                }))
                .collect::<Vec<_>>(),
        },
        "endpoints": endpoints
            .iter()
            .map(|endpoint| serde_json::json!({
                "path": endpoint.path,
                "name": endpoint.name,
                "ready": endpoint.ready,
                "provisioning": endpoint.provisioning,
            }))
            .collect::<Vec<_>>(),
    });
    #[cfg(feature = "stt")]
    let response = {
        let mut response = response;
        response["speech"] = serde_json::json!(crate::system::SpeechSnapshot::from(speech));
        response
    };
    Ok(Json(response))
}

#[cfg(test)]
#[path = "status-tests.rs"]
mod tests;
