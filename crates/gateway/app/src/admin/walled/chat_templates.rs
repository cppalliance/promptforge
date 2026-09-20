//! Read-only chat-template catalog and effective-resolution admin view.

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use gateway_config::{Config, LocalModelConfig, ModelKind};
use gateway_local::artifacts::existing_model_path;
use gateway_local::chat_templates::{Family, model_family_mappings};
use gateway_local::{
    ChatTemplateResolution, ChatTemplateSource, inspect_chat_template, resolve_cache_root,
};
use serde::Serialize;

use super::config_pending::load_pending_for_running;
use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, blocking};
use crate::registry::RouteInfo;

const CHAT_TEMPLATES: RouteInfo = RouteInfo::walled("/admin/chat-templates", &[Method::GET]);

/// The chat-template catalog route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[CHAT_TEMPLATES];

/// The chat-template catalog route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(CHAT_TEMPLATES.path, get(admin_chat_templates))
}

#[derive(Serialize)]
struct FamilyReply {
    slug: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct MappingReply {
    model_id: &'static str,
    family: &'static str,
}

#[derive(Serialize)]
struct ModelReply {
    name: String,
    effective_source: &'static str,
    effective_family: Option<&'static str>,
    detected_family: Option<&'static str>,
    reason: String,
}

#[derive(Serialize)]
struct CatalogReply {
    families: Vec<FamilyReply>,
    mappings: Vec<MappingReply>,
    models: Vec<ModelReply>,
}

/// Serves bundled families, exact model mappings, and pending-model decisions.
pub(crate) async fn admin_chat_templates(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let (running, running_profile) = {
        let live = state.live.read().await;
        (Arc::clone(&live.config), live.profile_name.clone())
    };
    let config_path = state.config.as_ref().map(|config| config.path.clone());
    let reply = blocking(move || {
        let config = match config_path {
            Some(path) => load_pending_for_running(&path, running_profile.as_deref())?,
            None => (*running).clone(),
        };
        serialize_catalog(&config)
    })
    .await??;
    Ok(Json(reply))
}

fn serialize_catalog(config: &Config) -> Result<serde_json::Value, GatewayError> {
    let families = Family::ALL
        .into_iter()
        .map(|family| FamilyReply {
            slug: family.canonical_name(),
            label: family.display_label(),
        })
        .collect();
    let mappings = model_family_mappings()
        .iter()
        .map(|(model_id, family)| MappingReply {
            model_id,
            family: family.canonical_name(),
        })
        .collect();
    let cache_root = resolve_cache_root(config.local().cache_dir());
    let models = config
        .catalog_local_models()
        .iter()
        .filter(|model| model.kind() == ModelKind::Chat)
        .map(|model| model_reply(model, cache_root.as_deref()))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_value(CatalogReply {
        families,
        mappings,
        models,
    })
    .map_err(|error| GatewayError::PendingConfig(error.to_string()))
}

fn model_reply(
    model: &LocalModelConfig,
    cache_root: Result<&Path, &gateway_local::LocalError>,
) -> Result<ModelReply, GatewayError> {
    let (model_path, inspection_error) = match cache_root {
        Ok(root) => match existing_model_path(root, model.source()) {
            Ok(path) => (path, None),
            Err(error) => (None, Some(error.to_string())),
        },
        Err(error) => (None, Some(error.to_string())),
    };
    let resolution = match inspect_chat_template(model, model_path.as_deref()) {
        Ok(resolution) => resolution,
        Err(error) => {
            let fallback = inspect_chat_template(model, None)
                .map_err(|fallback| GatewayError::ModelInfo(Box::new(fallback)))?;
            return Ok(resolution_reply(model, &fallback, Some(error.to_string())));
        }
    };
    Ok(resolution_reply(model, &resolution, inspection_error))
}

fn resolution_reply(
    model: &LocalModelConfig,
    resolution: &ChatTemplateResolution,
    inspection_error: Option<String>,
) -> ModelReply {
    let reason = match (resolution.source(), inspection_error) {
        (ChatTemplateSource::Builtin | ChatTemplateSource::Custom, _) => {
            resolution.reason().to_owned()
        }
        (_, Some(error)) => format!(
            "{} Artifact inspection was unavailable: {error}.",
            resolution.reason()
        ),
        (_, None) => resolution.reason().to_owned(),
    };
    ModelReply {
        name: model.name().to_owned(),
        effective_source: resolution.source().as_str(),
        effective_family: resolution.family().map(Family::canonical_name),
        detected_family: resolution.detected_family().map(Family::canonical_name),
        reason,
    }
}

#[cfg(test)]
#[path = "chat_templates-tests.rs"]
mod tests;
