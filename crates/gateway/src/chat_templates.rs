//! Read-only chat-template catalog and effective-resolution admin view.

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use gateway_config::{Config, LocalModelConfig, ModelKind};
use gateway_local::artifacts::existing_model_path;
use gateway_local::chat_templates::{Family, model_family_mappings};
use gateway_local::{
    ChatTemplateResolution, ChatTemplateSource, inspect_chat_template, resolve_cache_root,
};
use serde::Serialize;

use crate::config_pending::load_pending_for_running;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

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
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let (running, running_profile) = {
        let live = state.live.read().await;
        (Arc::clone(&live.config), live.profile_name.clone())
    };
    let config_path = state.config.as_ref().map(|config| config.path.clone());
    let reply = tokio::task::spawn_blocking(move || {
        let config = match config_path {
            Some(path) => load_pending_for_running(&path, running_profile.as_deref())?,
            None => (*running).clone(),
        };
        serialize_catalog(&config)
    })
    .await
    .map_err(|join| GatewayError::PendingConfig(join.to_string()))??;
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
mod tests {
    use gateway_config::Config;

    use crate::test_support::serve;

    const CONFIG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "template-key"

[[local_model]]
name = "mapped-auto"
kind = "chat"
description = "mapped model"
source = "https://huggingface.co/qwen/qwen3-8b/resolve/main/model.gguf"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
context = 4096

[[local_model]]
name = "known-broken"
kind = "chat"
description = "known override"
source = "https://huggingface.co/unsloth/gemma-4-e2b-it-GGUF/resolve/main/model.gguf"
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
context = 4096

[[local_model]]
name = "builtin"
kind = "chat"
description = "built in"
source = "models/builtin.gguf"
context = 4096
chat_template_file = "builtin:phi-4"

[[local_model]]
name = "custom"
kind = "chat"
description = "custom path"
source = "models/custom.gguf"
context = 4096
chat_template_file = "templates/custom.jinja"
"#;

    #[tokio::test]
    async fn catalog_requires_the_gateway_bearer() {
        let config = Config::from_toml_str(CONFIG).expect("config parses");
        let addr = serve(config).await;

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/chat-templates"))
            .send()
            .await
            .expect("request sends");

        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn catalog_serializes_labels_mappings_and_effective_resolutions() {
        let config = Config::from_toml_str(CONFIG).expect("config parses");
        let addr = serve(config).await;

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/chat-templates"))
            .bearer_auth("template-key")
            .send()
            .await
            .expect("request sends");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("body is JSON");
        assert_eq!(body["families"][0]["slug"], "chatml");
        assert_eq!(body["families"][0]["label"], "ChatML");
        assert!(
            body["mappings"]
                .as_array()
                .expect("mappings array")
                .iter()
                .any(|mapping| {
                    mapping["model_id"] == "qwen/qwen3-8b" && mapping["family"] == "qwen-3"
                })
        );
        let models = body["models"].as_array().expect("models array");
        let named = |name: &str| {
            models
                .iter()
                .find(|model| model["name"] == name)
                .expect("named model")
        };
        assert_eq!(named("mapped-auto")["effective_source"], "embedded");
        assert_eq!(named("mapped-auto")["detected_family"], "qwen-3");
        assert_eq!(named("known-broken")["effective_source"], "known-override");
        assert!(
            named("known-broken")["reason"]
                .as_str()
                .expect("reason")
                .contains("Known-broken")
        );
        assert_eq!(named("builtin")["effective_family"], "phi-4");
        assert_eq!(named("custom")["effective_source"], "custom");
    }
}
