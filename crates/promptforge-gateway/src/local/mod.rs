//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](crate::routing::Model). Dropping
//! [`LocalRuntime`] kills the children.

mod artifacts;
mod error;
mod server;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::config::{Config, LocalModelConfig, ThinkingMode};
use crate::queue::{EndpointLane, QueueConfig};
use crate::routing::{Endpoint, Model};
use crate::upstream::OpenAiUpstream;

pub use artifacts::{
    DEV_MODEL_NAME, DEV_MODEL_SHA256, DEV_MODEL_URL, SCENARIO_MODEL_NAME, SCENARIO_MODEL_SHA256,
    SCENARIO_MODEL_URL,
};
pub use error::LocalError;

use artifacts::{ArtifactStore, default_promptforge_root};
use server::{LaunchOptions, ServerGuard};

/// Running local `llama-server` children and the models they back.
///
/// Keep this value alive for the lifetime of the gateway process. Dropping it
/// terminates every child.
#[derive(Debug)]
pub struct LocalRuntime {
    guards: Vec<ServerGuard>,
    models: Vec<Arc<Model>>,
}

impl LocalRuntime {
    /// Provisions binaries/models and starts one `llama-server` per local model.
    ///
    /// When `config.local_models` is empty, returns an empty runtime without
    /// downloading anything.
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub fn start(config: &Config) -> Result<LocalRuntime, LocalError> {
        if config.local_models.is_empty() {
            return Ok(LocalRuntime {
                guards: Vec::new(),
                models: Vec::new(),
            });
        }

        let cache_root = resolve_cache_root(config.local.cache_dir.as_deref());
        tracing::info!(path = %cache_root.display(), "local model cache");
        let store = ArtifactStore::new(cache_root)?;
        let llama_server = store.provision_llama_server()?;
        tracing::info!(path = %llama_server.display(), "provisioned llama-server");

        let interrupted = Arc::new(AtomicBool::new(false));
        arm_startup_interrupt(Arc::clone(&interrupted));
        let mut guards = Vec::with_capacity(config.local_models.len());
        let mut models = Vec::with_capacity(config.local_models.len());

        for local_model in &config.local_models {
            let model_path =
                store.ensure_model(&local_model.source, local_model.sha256.as_deref())?;
            tracing::info!(
                model = %local_model.name,
                path = %model_path.display(),
                "provisioned local GGUF"
            );

            let options = launch_options(local_model);
            let guard =
                ServerGuard::start(&llama_server, &model_path, &options, interrupted.as_ref())?;
            let endpoint_id = format!("local-{}", local_model.name);
            let upstream = Arc::new(OpenAiUpstream::new(
                &guard.base_url(),
                crate::config::Secret::from(guard.api_key().to_owned()),
            ));
            let lane = EndpointLane::new(1, &QueueConfig::default());
            let endpoint = Arc::new(Endpoint {
                id: endpoint_id,
                upstream,
                lane,
            });
            models.push(Arc::new(Model {
                name: local_model.name.clone(),
                description: local_model.description.clone(),
                context: local_model.context,
                thinking: local_model.thinking,
                upstream_name: guard.model_alias().to_owned(),
                endpoint,
            }));
            tracing::info!(
                model = %local_model.name,
                base_url = %guard.base_url(),
                "local llama-server ready"
            );
            guards.push(guard);
        }

        Ok(LocalRuntime { guards, models })
    }

    /// Models registered for local inference, in `[[local_model]]` order.
    #[must_use]
    pub fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    /// Number of running `llama-server` children.
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.guards.len()
    }
}

fn resolve_cache_root(configured: Option<&str>) -> PathBuf {
    match configured {
        Some(path) if !path.is_empty() => expand_configured_path(path),
        _ => default_promptforge_root(),
    }
}

fn expand_configured_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return artifacts::default_home().join(rest);
    }
    if let Some(rest) = path.strip_prefix("~\\") {
        return artifacts::default_home().join(rest);
    }
    if path == "~" {
        return artifacts::default_home();
    }
    PathBuf::from(path)
}

fn launch_options(model: &LocalModelConfig) -> LaunchOptions {
    LaunchOptions {
        ctx_size: model.context,
        n_predict: model.n_predict,
        gpu_layers: model.gpu_layers,
        flash_attention: model.flash_attention,
        cache_type_k: model.cache_type_k.clone(),
        cache_type_v: model.cache_type_v.clone(),
        think: !matches!(model.thinking, ThinkingMode::Never),
    }
}

/// Arms a Ctrl-C watcher so readiness loops can abort while children are starting.
fn arm_startup_interrupt(interrupted: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async {
            if tokio::signal::ctrl_c().await.is_ok() {
                interrupted.store(true, Ordering::Release);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn empty_local_models_starts_noop_runtime() {
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "remote"
context = 8192
upstream = "u"
endpoints = ["e"]
"#,
        )
        .expect("config");
        let runtime = LocalRuntime::start(&config).expect("empty local runtime");
        assert_eq!(runtime.child_count(), 0);
        assert!(runtime.models().is_empty());
    }
}
