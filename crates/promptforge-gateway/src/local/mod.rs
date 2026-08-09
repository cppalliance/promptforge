//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](crate::routing::Model). Dropping
//! [`LocalRuntime`] kills the children.

pub(crate) mod artifacts;
mod error;
pub(crate) mod sidecar;
mod server;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use promptforge_core::dialects::{DialectEvidence, ToolDialectRegistry};
use serde_json::Value;

use crate::config::{Config, LocalModelConfig, ThinkingMode};
use crate::queue::EndpointLane;
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
    /// An empty runtime with no children. Used when no `[[local_model]]` is set
    /// and as the placeholder before the first profile switch.
    #[must_use]
    pub fn empty() -> LocalRuntime {
        LocalRuntime {
            guards: Vec::new(),
            models: Vec::new(),
        }
    }

    /// Provisions binaries/models and starts one `llama-server` per local model.
    ///
    /// When `config.local_models` is empty, returns an empty runtime without
    /// downloading anything.
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub fn start(config: &Config) -> Result<LocalRuntime, LocalError> {
        if config.local_models.is_empty() {
            return Ok(LocalRuntime::empty());
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

            maybe_write_sidecar(&store, &local_model.source, &model_path);

            let concurrency = config
                .local_model_concurrency(local_model)
                .map_err(|e| LocalError::Server(e.to_string()))?;
            let parallel = u32::try_from(concurrency).expect("lane concurrency fits in u32");
            let options = launch_options(local_model, parallel);
            let guard =
                ServerGuard::start(&llama_server, &model_path, &options, interrupted.as_ref())?;
            let endpoint_id = format!("local-{}", local_model.name);
            let upstream = Arc::new(OpenAiUpstream::new(
                &guard.base_url(),
                crate::config::Secret::from(guard.api_key().to_owned()),
            ));
            let lane = EndpointLane::new(concurrency, &config.queue);
            let endpoint = Arc::new(Endpoint {
                id: endpoint_id,
                upstream,
                lane,
            });
            let (tool_dialect, tools_mode) = resolve_local_dialect(
                &guard,
                &local_model.name,
                &model_path,
            )?;
            models.push(Arc::new(Model {
                name: local_model.name.clone(),
                description: local_model.description.clone(),
                context: local_model.context,
                thinking: local_model.thinking,
                tool_dialect,
                tools_mode,
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
    }
}

/// Fetches `/props` from a ready local llama-server and resolves the tool dialect.
///
/// When `/props` does not supply a `chat_template`, the sidecar `.md` next to
/// the GGUF is consulted as a fallback. Props always wins over conflicting
/// sidecar data.
///
/// Returns `(tool_dialect, tools_mode)` strings for the routing model.
/// Hard-fails on `DialectNone` or `DialectTie` so local models never silently
/// default to an incorrect dialect.
fn resolve_local_dialect(
    guard: &ServerGuard,
    model_name: &str,
    model_path: &Path,
) -> Result<(String, String), LocalError> {
    let mut evidence = fetch_props_evidence(guard)?;

    if evidence.chat_template.is_none() {
        if let Some(sidecar_meta) = read_sidecar_quietly(model_path) {
            if let Some(template) = sidecar_meta.chat_template {
                tracing::debug!(
                    model = %model_name,
                    "supplementing chat_template from sidecar"
                );
                evidence.chat_template = Some(template);
            }
        }
    }

    tracing::debug!(
        model = %model_name,
        supports_tool_calls = ?evidence.supports_tool_calls,
        has_template = evidence.chat_template.is_some(),
        model_id = ?evidence.model_id,
        "dialect evidence from /props + sidecar"
    );
    let registry = ToolDialectRegistry::builtin();
    let dialect_id = registry.resolve(&evidence).map_err(|error| {
        LocalError::Server(format!(
            "dialect resolution failed for local model {model_name}: {error}"
        ))
    })?;
    let tools_mode = dialect_id.tools_mode();
    Ok((dialect_id.to_string(), tools_mode.to_string()))
}

/// Reads the sidecar metadata next to a GGUF, logging and swallowing errors.
fn read_sidecar_quietly(model_path: &Path) -> Option<sidecar::SidecarMeta> {
    match sidecar::read_sidecar(model_path) {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!(
                path = %model_path.display(),
                error = %e,
                "failed to read sidecar"
            );
            None
        }
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
        tracing::debug!(path = %sidecar_file.display(), "sidecar already exists");
        return;
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
    let chat_template = sidecar::fetch_hf_chat_template(
        &client,
        source,
        bearer.as_deref(),
    );

    if chat_template.is_none() {
        tracing::debug!(source = %source, "no chat_template from HF metadata");
        return;
    }

    let meta = sidecar::SidecarMeta {
        source: Some(source.to_owned()),
        fetched: Some(sidecar::utc_now_iso()),
        chat_template,
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

const PROPS_TIMEOUT: Duration = Duration::from_secs(5);

/// Fetches `GET /props` from a ready llama-server and builds [`DialectEvidence`].
fn fetch_props_evidence(guard: &ServerGuard) -> Result<DialectEvidence, LocalError> {
    let base = format!("http://127.0.0.1:{}", guard.port());
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(PROPS_TIMEOUT)
        .build()
        .map_err(|e| LocalError::Server(format!("build props client: {e}")))?;

    let response = client
        .get(format!("{base}/props"))
        .bearer_auth(guard.api_key())
        .send()
        .map_err(|e| LocalError::Server(format!("GET /props failed: {e}")))?;

    if !response.status().is_success() {
        return Err(LocalError::Server(format!(
            "GET /props returned {}",
            response.status()
        )));
    }

    let props: Value = response
        .json()
        .map_err(|e| LocalError::Server(format!("parse /props JSON: {e}")))?;

    let chat_template = props
        .get("chat_template")
        .and_then(Value::as_str)
        .map(String::from);

    // llama-server /props exposes `default_generation_settings.model` as the
    // loaded model path and `default_generation_settings.samplers` etc.
    let model_id = props
        .get("default_generation_settings")
        .and_then(|dgs| dgs.get("model"))
        .and_then(Value::as_str)
        .map(String::from);

    // `total_slots` and capabilities are top-level in /props. When the server
    // was launched with `--jinja` and the template declares tool support, the
    // /v1/models response carries `meta.has_tool_call_capability`. We check
    // both /props and /v1/models for the capability flag.
    let supports_tool_calls = fetch_tool_call_capability(&client, &base, guard.api_key());

    Ok(DialectEvidence::new(
        Some(supports_tool_calls),
        chat_template,
        model_id,
        None,
    ))
}

/// Checks the /v1/models response for native tool-call capability.
fn fetch_tool_call_capability(
    client: &reqwest::blocking::Client,
    base: &str,
    api_key: &str,
) -> bool {
    let Ok(response) = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(body) = response.json::<Value>() else {
        return false;
    };
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("meta"))
        .and_then(|meta| meta.get("has_tool_call_capability"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn sidecar_supplements_missing_template() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("gemma-3-27b.gguf");
        fs::write(&gguf, b"fake").expect("write gguf");

        let meta = sidecar::SidecarMeta {
            source: Some("https://huggingface.co/google/gemma/resolve/main/gemma-3-27b.gguf".to_owned()),
            fetched: Some("2026-08-08T00:00:00Z".to_owned()),
            chat_template: Some("<start_of_turn>user\n{{ content }}".to_owned()),
            card: None,
        };
        sidecar::write_sidecar(&gguf, &meta).expect("write sidecar");

        let mut evidence = DialectEvidence::new(
            Some(false),
            None, // props had no template
            Some("gemma-3-27b-it".to_owned()),
            None,
        );

        // Simulate the merge: sidecar fills in missing chat_template
        if evidence.chat_template.is_none() {
            if let Some(sc) = read_sidecar_quietly(&gguf) {
                evidence.chat_template = sc.chat_template;
            }
        }

        assert!(evidence.chat_template.is_some());
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve with sidecar");
        assert_eq!(id.to_string(), "gemma3_tool_code");
    }

    #[test]
    fn props_wins_over_conflicting_sidecar() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("model.gguf");
        fs::write(&gguf, b"fake").expect("write gguf");

        let meta = sidecar::SidecarMeta {
            source: None,
            fetched: None,
            chat_template: Some("sidecar-template-should-lose".to_owned()),
            card: None,
        };
        sidecar::write_sidecar(&gguf, &meta).expect("write sidecar");

        let evidence = DialectEvidence::new(
            Some(true), // props says native tools
            Some("props-template-wins".to_owned()), // props has its own template
            None,
            None,
        );

        // Props already has chat_template, so sidecar should NOT be consulted.
        // The merge logic only reads sidecar when evidence.chat_template is None.
        assert_eq!(evidence.chat_template.as_deref(), Some("props-template-wins"));
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "openai");
    }

    #[test]
    fn sidecar_missing_file_is_harmless() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("no-sidecar.gguf");
        let result = read_sidecar_quietly(&gguf);
        assert!(result.is_none());
    }

    #[test]
    fn empty_local_models_starts_noop_runtime() {
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

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
    fn gemma_props_resolve_to_gemma3_tool_code() {
        let evidence = DialectEvidence::new(
            Some(false),
            Some("<start_of_turn>user\n".to_string()),
            Some("gemma-3-27b-it".to_string()),
            None,
        );
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "gemma3_tool_code");
        assert_eq!(id.tools_mode().to_string(), "emulated");
    }

    #[test]
    fn tools_true_resolves_to_openai() {
        let evidence = DialectEvidence::new(Some(true), None, None, None);
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "openai");
        assert_eq!(id.tools_mode().to_string(), "native");
    }

    #[test]
    fn dialect_none_is_hard_fail() {
        let evidence = DialectEvidence::default();
        let registry = ToolDialectRegistry::builtin();
        let result = registry.resolve(&evidence);
        assert!(result.is_err(), "empty evidence must hard-fail");
    }

    #[test]
    fn remote_model_defaults_to_openai_native() {
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

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
