//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](crate::routing::Model). Dropping
//! [`LocalRuntime`] kills the children.

pub(crate) mod artifacts;
mod dialect;
mod error;
mod server;
pub(crate) mod sidecar;
mod upstream;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use crate::queue::EndpointLane;
use crate::routing::{Endpoint, Model};
use promptforge_gateway_config::{Config, LocalModelConfig, ThinkingMode};

pub(crate) use error::LocalError;

use artifacts::ArtifactStore;
use dialect::resolve_local_dialect;
use server::{LaunchOptions, ServerGuard};
use upstream::LocalUpstream;

/// Running local `llama-server` children and the models they back.
///
/// Keep this value alive for the lifetime of the gateway process. Dropping it
/// terminates every child (via [`LocalUpstream`] Drop → [`ServerGuard`] Drop).
#[derive(Debug)]
pub(crate) struct LocalRuntime {
    models: Vec<Arc<Model>>,
}

impl LocalRuntime {
    /// An empty runtime with no children. Used when no `[[local_model]]` is set
    /// and as the placeholder before the first profile switch.
    #[must_use]
    pub(crate) fn empty() -> LocalRuntime {
        LocalRuntime { models: Vec::new() }
    }

    /// Provisions binaries/models and starts one `llama-server` per local model.
    ///
    /// When `config.local_models` is empty, returns an empty runtime without
    /// downloading anything.
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub(crate) fn start(config: &Config) -> Result<LocalRuntime, LocalError> {
        if config.local_models.is_empty() {
            return Ok(LocalRuntime::empty());
        }

        let cache_root = resolve_cache_root(config.local.cache_dir.as_deref())?;
        tracing::info!(path = %cache_root.display(), "local model cache");
        let store = ArtifactStore::new(cache_root)?;
        let llama_server = store.provision_llama_server()?;
        tracing::info!(path = %llama_server.display(), "provisioned llama-server");

        let interrupted = startup_interrupt_flag();
        let mut models = Vec::with_capacity(config.local_models.len());

        for local_model in &config.local_models {
            let model_path =
                store.ensure_model(&local_model.source, local_model.sha256.as_deref())?;
            tracing::info!(
                model = %local_model.name,
                path = %model_path.display(),
                "provisioned local GGUF"
            );

            maybe_write_sidecar(&store, &local_model.source, &model_path);

            let concurrency = config
                .local_model_concurrency(local_model)
                .map_err(|source| LocalError::LaneConcurrency {
                    model: local_model.name.clone(),
                    source,
                })?;
            let parallel =
                u32::try_from(concurrency).map_err(|_| LocalError::LaneTooLarge { concurrency })?;
            let options = launch_options(local_model, parallel);
            let guard =
                ServerGuard::start(&llama_server, &model_path, &options, interrupted.as_ref())?;
            let endpoint_id = format!("local-{}", local_model.name);
            let (tool_dialect, tools_mode) =
                resolve_local_dialect(&guard, &local_model.name, &model_path)?;
            let upstream_name = guard.model_alias().to_owned();
            let base_url = guard.base_url();
            let upstream = Arc::new(LocalUpstream::new(
                guard,
                llama_server.clone(),
                model_path.clone(),
                options,
                local_model.name.clone(),
            ));
            let lane = EndpointLane::new(concurrency, &config.queue);
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
                tool_dialect,
                tools_mode,
                upstream_name,
                endpoint,
            }));
            tracing::info!(
                model = %local_model.name,
                base_url = %base_url,
                "local llama-server ready"
            );
        }

        Ok(LocalRuntime { models })
    }

    /// Models registered for local inference, in `[[local_model]]` order.
    #[must_use]
    pub(crate) fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    /// Number of local model endpoints (each owns one `llama-server` child).
    #[must_use]
    pub(crate) fn child_count(&self) -> usize {
        self.models.len()
    }

    /// Explicitly terminate every owned `llama-server` child and disable respawn,
    /// returning the first teardown failure after attempting *all* children.
    ///
    /// Dropping the runtime does not guarantee child termination, because the
    /// routing table holds `Arc<dyn Upstream>` clones of these same models, so
    /// the runtime is not the sole owner (PFGL-MOD-001). This drives an explicit
    /// teardown through the [`Upstream`](crate::upstream::Upstream) seam so a
    /// profile switch frees the old children's VRAM deterministically before the
    /// replacement profile's children start. Every child is torn down even if an
    /// earlier one fails, so one stuck child never strands the rest.
    ///
    /// # Errors
    /// Returns the first [`LocalError`] a child teardown produced.
    pub(crate) fn shutdown(&self) -> Result<(), LocalError> {
        let mut first_error: Option<LocalError> = None;
        for model in &self.models {
            if let Err(error) = model.endpoint.upstream.shutdown() {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn resolve_cache_root(configured: Option<&str>) -> Result<PathBuf, LocalError> {
    match configured {
        Some(path) if !path.is_empty() => expand_configured_path(path),
        // An unset cache_dir defaults to `~/.promptforge`; a missing home is a
        // typed error rather than a silent working-directory fallback (ART-009).
        _ => artifacts::default_promptforge_root_checked(),
    }
}

fn expand_configured_path(path: &str) -> Result<PathBuf, LocalError> {
    if let Some(rest) = path.strip_prefix("~/") {
        return Ok(artifacts::default_home_checked()?.join(rest));
    }
    if let Some(rest) = path.strip_prefix("~\\") {
        return Ok(artifacts::default_home_checked()?.join(rest));
    }
    if path == "~" {
        return artifacts::default_home_checked();
    }
    Ok(PathBuf::from(path))
}

fn launch_options(model: &LocalModelConfig, parallel: u32) -> LaunchOptions {
    LaunchOptions {
        ctx_size: model.context,
        n_predict: model.n_predict,
        parallel,
        gpu_layers: model.gpu_layers,
        flash_attention: model.flash_attention,
        cache_type_k: model.cache_type_k.clone(),
        cache_type_v: model.cache_type_v.clone(),
        think: !matches!(model.thinking, ThinkingMode::Never),
        chat_template_file: model.chat_template_file.as_ref().map(PathBuf::from),
    }
}

/// Best-effort: fetch HF metadata and write a sidecar `.md` beside the GGUF.
///
/// Only attempts the fetch for HF URLs. Failures are logged at debug level
/// and swallowed - the sidecar is supplementary, never required.
fn maybe_write_sidecar(_store: &ArtifactStore, source: &str, model_path: &Path) {
    if !source.starts_with("https://huggingface.co/") {
        return;
    }
    let sidecar_file = sidecar::sidecar_path(model_path);
    if sidecar_file.is_file() {
        // Validate the existing sidecar rather than blindly trusting the file's
        // presence (SIDECAR-004): only skip the refetch when it reads back as a
        // current, usable sidecar. An unversioned, oversized, or template-less
        // sidecar falls through and is rewritten.
        match sidecar::read_sidecar(model_path) {
            Ok(Some(meta)) if meta.chat_template.is_some() => {
                tracing::debug!(path = %sidecar_file.display(), "valid sidecar already exists");
                return;
            }
            _ => {
                tracing::debug!(
                    path = %sidecar_file.display(),
                    "existing sidecar invalid or incomplete; refetching"
                );
            }
        }
    }

    let client = match reqwest::blocking::Client::builder()
        .user_agent(concat!("promptforge-gateway/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "could not build sidecar HTTP client");
            return;
        }
    };

    let bearer = artifacts::hub_bearer_token_from_env();
    let chat_template = match sidecar::fetch_hf_chat_template(&client, source, bearer.as_deref()) {
        Ok(Some(template)) => template,
        Ok(None) => {
            tracing::debug!(source = %source, "no chat_template from HF metadata");
            return;
        }
        Err(error) => {
            // Deliberate downgrade (SIDECAR-006): the sidecar is supplementary,
            // so a fetch failure is logged and skipped, not propagated.
            tracing::debug!(source = %source, error = %error, "sidecar fetch failed");
            return;
        }
    };

    let meta = sidecar::SidecarMeta {
        source: Some(source.to_owned()),
        fetched: Some(sidecar::utc_now_iso()),
        chat_template: Some(chat_template),
        card: None,
    };

    if let Err(e) = sidecar::write_sidecar(model_path, &meta) {
        tracing::debug!(
            path = %sidecar_file.display(),
            error = %e,
            "failed to write sidecar"
        );
    } else {
        tracing::info!(path = %sidecar_file.display(), "wrote HF metadata sidecar");
    }
}

/// Process-wide Ctrl-C flag for startup readiness loops.
static STARTUP_INTERRUPT: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Returns the shared startup-interrupt flag, installing the single process-wide
/// Ctrl-C watcher on first use.
///
/// Earlier code armed a fresh OS thread and Tokio runtime on every
/// [`LocalRuntime::start`], leaking both on each profile switch. One `OnceLock`
/// watcher is installed once and its flag shared by every start.
fn startup_interrupt_flag() -> Arc<AtomicBool> {
    STARTUP_INTERRUPT
        .get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(false));
            let watcher = Arc::clone(&flag);
            thread::spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        watcher.store(true, Ordering::Release);
                    }
                });
            });
            flag
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use promptforge_gateway_config::Config;

    #[test]
    fn empty_local_models_starts_noop_runtime() {
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

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

    #[test]
    fn remote_model_defaults_to_openai_native() {
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "remote"
description = "a remote model"
context = 8192
upstream = "u"
endpoints = ["e"]
"#,
        )
        .expect("config");
        let routing = crate::routing::Routing::from_config(&config).unwrap();
        let model = routing.model("remote").unwrap();
        assert_eq!(model.tool_dialect, "openai");
        assert_eq!(model.tools_mode, "native");
    }
}
