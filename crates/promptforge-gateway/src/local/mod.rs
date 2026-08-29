//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](crate::routing::Model). Dropping
//! [`LocalRuntime`] kills the children.

pub(crate) mod artifacts;
pub(crate) mod cache;
mod dialect;
mod error;
mod server;
pub(crate) mod sidecar;
mod upstream;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use crate::queue::DominionQueue;
use crate::routing::{Endpoint, Model, dominion_queues};
use promptforge_gateway_config::{Config, LocalModelConfig, ModelKind, QueuePolicy, ThinkingMode};

pub(crate) use error::LocalError;

use artifacts::ArtifactStore;
use dialect::resolve_local_dialect;
use server::{LaunchOptions, ServeMode, ServerGuard, SpeculativeLaunch};
use upstream::LocalUpstream;

/// Running local `llama-server` children and the models they back.
///
/// Keep this value alive for the lifetime of the gateway process. Dropping it
/// terminates every child (via [`LocalUpstream`] Drop → [`ServerGuard`] Drop).
#[derive(Debug)]
pub(crate) struct LocalRuntime {
    models: Vec<Arc<Model>>,
    /// The upstreams behind `models`, kept un-erased so diagnostics can reach
    /// each child's captured output.
    upstreams: Vec<LocalUpstream>,
    /// The profile's `[local].cache_dir`, retained so the `/v1/cache` routes
    /// resolve the same root provisioning does, even with no local models.
    cache_dir: Option<String>,
}

impl LocalRuntime {
    /// An empty runtime with no children. Used when no `[[local_model]]` is set
    /// and as the placeholder before the first profile switch.
    #[must_use]
    pub(crate) fn empty() -> LocalRuntime {
        LocalRuntime {
            models: Vec::new(),
            upstreams: Vec::new(),
            cache_dir: None,
        }
    }

    /// Provisions binaries/models and starts one `llama-server` per local model.
    ///
    /// When the config declares no `[[local_model]]`, returns an empty runtime
    /// without downloading anything.
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub(crate) fn start(config: &Config) -> Result<LocalRuntime, LocalError> {
        let cache_dir = config.local().cache_dir().map(str::to_owned);
        if config.local_models().is_empty() {
            return Ok(LocalRuntime {
                models: Vec::new(),
                upstreams: Vec::new(),
                cache_dir,
            });
        }

        let cache_root = resolve_cache_root(config.local().cache_dir())?;
        tracing::info!(path = %cache_root.display(), "local model cache");
        let store = ArtifactStore::new(cache_root)?;
        let server = store.provision_llama_server()?;
        tracing::info!(path = %server.executable.display(), "provisioned llama-server");

        let interrupted = startup_interrupt_flag();
        let dominion_queues = dominion_queues(config);
        let mut models = Vec::with_capacity(config.local_models().len());
        let mut upstreams = Vec::with_capacity(config.local_models().len());

        for local_model in config.local_models() {
            let model_path = store.ensure_model(local_model.source(), local_model.sha256())?;
            tracing::info!(
                model = %local_model.name(),
                path = %model_path.display(),
                "provisioned local GGUF"
            );

            maybe_write_sidecar(&store, local_model.source(), &model_path);

            let admission = resolve_admission(&dominion_queues, local_model)?;
            let mut options = launch_options(local_model, admission.parallel);
            options.path_prefix.clone_from(&server.path_prefix);
            provision_companions(&store, local_model, &mut options)?;
            let guard = ServerGuard::start(
                &server.executable,
                &model_path,
                &options,
                interrupted.as_ref(),
            )?;
            let endpoint_id = format!("local-{}", local_model.name());
            // A non-chat child has no chat completions to dialect-match: like a
            // remote model, it carries the OpenAI default rather than hard-failing
            // on template-less `/props` evidence.
            let tool_dialect = match local_model.kind() {
                ModelKind::Chat => resolve_local_dialect(&guard, local_model.name(), &model_path)?,
                _ => "openai",
            };
            let upstream_name = guard.model_alias().to_owned();
            let base_url = guard.base_url();
            let upstream = LocalUpstream::new(
                guard,
                server.executable.clone(),
                model_path.clone(),
                options,
                local_model.name().to_owned(),
            );
            upstreams.push(upstream.clone());
            let upstream = Arc::new(upstream);
            let endpoint = Arc::new(Endpoint {
                id: endpoint_id,
                upstream,
                queue: admission.queue,
            });
            models.push(Arc::new(Model {
                name: local_model.name().to_owned(),
                kind: local_model.kind(),
                description: local_model.description().to_owned(),
                context: local_model.context(),
                thinking: local_model.thinking(),
                capabilities: local_model.capabilities().clone(),
                tool_dialect: tool_dialect.to_owned(),
                upstream_name,
                endpoint,
            }));
            tracing::info!(
                model = %local_model.name(),
                base_url = %base_url,
                "local llama-server ready"
            );
        }

        Ok(LocalRuntime {
            models,
            upstreams,
            cache_dir,
        })
    }

    /// Bounded captured-output tails of the running local children, keyed by
    /// configured model name.
    #[must_use]
    pub(crate) fn diagnostics(&self) -> Vec<(String, String)> {
        self.upstreams
            .iter()
            .map(|upstream| (upstream.model_name().to_owned(), upstream.diagnostics()))
            .collect()
    }

    /// Models registered for local inference, in `[[local_model]]` order.
    #[must_use]
    pub(crate) fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    /// The profile's configured `[local].cache_dir`, when set.
    #[must_use]
    pub(crate) fn cache_dir(&self) -> Option<&str> {
        self.cache_dir.as_deref()
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

/// Resolves the operator cache root from the configured `[local].cache_dir`,
/// defaulting to `~/.promptforge` (ART-009).
///
/// # Errors
/// Returns [`LocalError::MissingHome`] when no cache dir is configured and the
/// home variable is unset or empty.
pub(crate) fn resolve_cache_root(configured: Option<&str>) -> Result<PathBuf, LocalError> {
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

/// The admission wiring resolved for one local model: the child's
/// `--parallel` value and the queue the model's endpoint admits through.
struct LocalAdmission {
    parallel: u32,
    queue: DominionQueue,
}

/// Resolve a local model's admission wiring.
///
/// The `--parallel` value is `LocalModelConfig::parallel` (default 1). A
/// model without a `dominion` gets a per-model queue limited to that same
/// number, preserving the invariant that the child's `--parallel` and the
/// queue limit are one value. A model bound to a dominion instead admits
/// through that dominion's shared queue - one `Arc` shared with every other
/// bound local model - and the dominion's own limit governs admission.
fn resolve_admission(
    dominion_queues: &HashMap<&str, DominionQueue>,
    model: &LocalModelConfig,
) -> Result<LocalAdmission, LocalError> {
    let parallel = model.parallel();
    let queue = match model.dominion() {
        Some(dominion_id) => dominion_queues.get(dominion_id).cloned().ok_or_else(|| {
            LocalError::UnknownDominion {
                model: model.name().to_owned(),
                dominion: dominion_id.to_owned(),
            }
        })?,
        // The unbound per-model queue uses the dominion defaults for depth
        // and fairness; the reject policy only exists on dominions.
        None => DominionQueue::new(parallel as usize, 100, true, QueuePolicy::Queue),
    };
    Ok(LocalAdmission { parallel, queue })
}

fn launch_options(model: &LocalModelConfig, parallel: u32) -> LaunchOptions {
    LaunchOptions {
        ctx_size: model.context(),
        n_predict: model.n_predict(),
        parallel,
        gpu_layers: model.gpu_layers(),
        flash_attention: model.flash_attention(),
        cache_type_k: model.cache_type_k().to_owned(),
        cache_type_v: model.cache_type_v().to_owned(),
        think: !matches!(model.thinking(), ThinkingMode::Never),
        chat_template_file: model.chat_template_file().map(PathBuf::from),
        serve_mode: match model.kind() {
            ModelKind::Embedding => ServeMode::Embeddings,
            ModelKind::Classifier => ServeMode::Reranking,
            // Chat (and any kind added after this mapping) launches with no flag.
            _ => ServeMode::Chat,
        },
        speculative: None,
        multimodal_projector: None,
        path_prefix: Vec::new(),
    }
}

/// Resolves a model's declared companions through the same `ensure_model`
/// machinery as the main model and records the owned paths in `options`.
///
/// Each companion lands in its own cache slot keyed by its own source
/// identity, with its own pin verified on hit and after download. Any
/// resolution failure returns before the caller spawns the child, so a bad
/// companion never becomes a spawned-then-failing server. A model without
/// companions leaves `options` untouched, preserving the exact command line
/// from before companions existed.
///
/// # Errors
/// Returns [`LocalError`] when a companion source cannot be resolved or its
/// pin does not match.
fn provision_companions(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    options: &mut LaunchOptions,
) -> Result<(), LocalError> {
    if let Some(speculative) = model.speculative() {
        let draft_model = store.ensure_model(speculative.source(), speculative.sha256())?;
        tracing::info!(
            model = %model.name(),
            path = %draft_model.display(),
            "provisioned speculative drafter GGUF"
        );
        options.speculative = Some(SpeculativeLaunch {
            draft_model,
            draft_max: speculative.draft_max().get(),
        });
    }
    if let Some(projector) = model.multimodal_projector() {
        let projector_path = store.ensure_model(projector.source(), projector.sha256())?;
        tracing::info!(
            model = %model.name(),
            path = %projector_path.display(),
            "provisioned multimodal projector GGUF"
        );
        options.multimodal_projector = Some(projector_path);
    }
    Ok(())
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
        assert!(runtime.diagnostics().is_empty());
    }

    #[test]
    fn remote_model_defaults_to_openai_dialect() {
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
    }

    #[tokio::test]
    async fn parallel_field_feeds_parallel_arg_and_queue_limit() {
        // A local model with `parallel = 3` launches its child with
        // `--parallel 3` (launch_options carries the number; the server tests
        // prove it renders into the argv) and admits at most 3 concurrent
        // requests through its per-model queue.
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
parallel = 3
"#,
        )
        .expect("config");
        let model = &config.local_models()[0];
        let queues = dominion_queues(&config);
        let admission = resolve_admission(&queues, model).expect("admission");

        assert_eq!(admission.parallel, 3);
        assert_eq!(launch_options(model, admission.parallel).parallel, 3);

        let _first = admission.queue.admit("client").await.unwrap();
        let _second = admission.queue.admit("client").await.unwrap();
        let third = admission.queue.admit("client").await.unwrap();

        // The fourth request exceeds the limit and parks as a waiter.
        let queue = admission.queue.clone();
        let blocked = tokio::spawn(async move { queue.admit("client").await });
        while admission.queue.waiter_count() != 1 {
            tokio::task::yield_now().await;
        }

        // Releasing a slot hands it to the parked waiter.
        drop(third);
        let _promoted = blocked.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn local_models_on_one_dominion_share_one_limit() {
        // Two local models bound to one local dominion compete for a single
        // pool of slots: filling the only slot through one model's binding
        // parks the other model's admit.
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 1

[[local_model]]
name = "a"
description = "model a"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"

[[local_model]]
name = "b"
description = "model b"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
"#,
        )
        .expect("config");
        let queues = dominion_queues(&config);
        let admission_a =
            resolve_admission(&queues, &config.local_models()[0]).expect("admission a");
        let admission_b =
            resolve_admission(&queues, &config.local_models()[1]).expect("admission b");
        // No `parallel` set: the child `--parallel` defaults to 1.
        assert_eq!(admission_a.parallel, 1);
        assert_eq!(admission_b.parallel, 1);

        let held = admission_a.queue.admit("client").await.unwrap();
        let queue_b = admission_b.queue.clone();
        let blocked = tokio::spawn(async move { queue_b.admit("client").await });
        while admission_a.queue.waiter_count() != 1 {
            tokio::task::yield_now().await;
        }

        drop(held);
        let _permit = blocked.await.unwrap().unwrap();
    }

    #[test]
    fn embedding_kind_sets_the_embeddings_launch_flag() {
        // `kind = "embedding"` maps to the child's `--embeddings` flag
        // (launch_options carries it; the server tests prove it renders into
        // the argv); a chat child launches without it.
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "embed"
kind = "embedding"
description = "a local embedding model"
source = "/models/embed.gguf"
context = 512

[[local_model]]
name = "chatty"
description = "a local chat model"
source = "/models/chat.gguf"
context = 4096
"#,
        )
        .expect("config");
        let embed = &config.local_models()[0];
        let chat = &config.local_models()[1];
        assert_eq!(launch_options(embed, 1).serve_mode, ServeMode::Embeddings);
        assert_eq!(launch_options(chat, 1).serve_mode, ServeMode::Chat);
    }

    #[test]
    fn classifier_kind_sets_the_reranking_launch_flag() {
        // `kind = "classifier"` maps to the child's `--reranking` flag
        // (launch_options carries it; the server tests prove it renders into
        // the argv); a chat child launches without it.
        let config = Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "rerank"
kind = "classifier"
description = "a local classifier model"
source = "/models/rerank.gguf"
context = 512

[[local_model]]
name = "chatty"
description = "a local chat model"
source = "/models/chat.gguf"
context = 4096
"#,
        )
        .expect("config");
        let classifier = &config.local_models()[0];
        let chat = &config.local_models()[1];
        assert_eq!(
            launch_options(classifier, 1).serve_mode,
            ServeMode::Reranking
        );
        assert_eq!(launch_options(chat, 1).serve_mode, ServeMode::Chat);
    }

    fn companion_config(body: &str) -> Config {
        Config::from_toml_str(&format!(
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
{body}"#
        ))
        .expect("config")
    }

    #[test]
    fn provision_companions_resolve_to_independent_pinned_slots() {
        // Each companion resolves through `ensure_model` under its own source
        // identity and its own pin: a shared verification state or a dropped
        // pin breaks the distinct markers, and a wiring slip breaks the
        // resolved paths or the carried draft maximum.
        use crate::testsupport::hex_sha256;

        let source_dir = tempfile::TempDir::new().expect("source dir");
        let draft = source_dir.path().join("draft.gguf");
        let projector = source_dir.path().join("mmproj.gguf");
        std::fs::write(&draft, b"draft-bytes").expect("write draft");
        std::fs::write(&projector, b"projector-bytes").expect("write projector");
        let config = companion_config(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = '{}'
sha256 = "{}"
draft_max = 2

[local_model.multimodal_projector]
source = '{}'
sha256 = "{}"
"#,
            draft.display(),
            hex_sha256(b"draft-bytes"),
            projector.display(),
            hex_sha256(b"projector-bytes"),
        ));
        let model = &config.local_models()[0];
        let temp = tempfile::TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(temp.path()).expect("store");

        let mut options = launch_options(model, 1);
        provision_companions(&store, model, &mut options).expect("provision companions");

        let speculative = options.speculative.expect("speculative launch state");
        assert_eq!(speculative.draft_model, draft);
        assert_eq!(speculative.draft_max, 2);
        assert_eq!(
            options.multimodal_projector.expect("projector path"),
            projector
        );

        // Each pinned path source records its own verification marker, keyed
        // by its own source identity.
        let draft_key = artifacts::source_cache_key(&draft.to_string_lossy());
        let projector_key = artifacts::source_cache_key(&projector.to_string_lossy());
        assert_ne!(draft_key, projector_key);
        let markers = temp.path().join("markers");
        assert!(markers.join(format!("{draft_key}.verified")).is_file());
        assert!(markers.join(format!("{projector_key}.verified")).is_file());
    }

    #[test]
    fn companion_provisioning_failures_precede_child_spawn() {
        // An unresolvable or pin-mismatching companion fails inside
        // `provision_companions`, which `LocalRuntime::start` calls before
        // `ServerGuard::start`: the error is a `LocalError` from provisioning,
        // never a spawned-then-failing server.
        use crate::testsupport::hex_sha256;

        let source_dir = tempfile::TempDir::new().expect("source dir");
        let draft = source_dir.path().join("draft.gguf");
        std::fs::write(&draft, b"real-draft-bytes").expect("write draft");
        let mismatching = companion_config(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = '{}'
sha256 = "{}"
draft_max = 2
"#,
            draft.display(),
            hex_sha256(b"different-bytes"),
        ));
        let temp = tempfile::TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(temp.path()).expect("store");
        let model = &mismatching.local_models()[0];
        let mut options = launch_options(model, 1);
        let error = provision_companions(&store, model, &mut options)
            .expect_err("pin mismatch must fail provisioning");
        assert!(matches!(error, LocalError::DigestMismatch { .. }));
        assert!(options.speculative.is_none());

        let missing = companion_config(
            r#"
[local_model.multimodal_projector]
source = "/definitely/not/a/real/mmproj.gguf"
"#,
        );
        let model = &missing.local_models()[0];
        let mut options = launch_options(model, 1);
        let error = provision_companions(&store, model, &mut options)
            .expect_err("a missing local source must fail provisioning");
        assert!(matches!(error, LocalError::InvalidSource { .. }));
        assert!(options.multimodal_projector.is_none());
    }

    #[test]
    fn model_without_companions_keeps_launch_options_unset() {
        // Provisioning is a no-op for a companion-less model: the options stay
        // exactly what `launch_options` produced, so the emitted command line
        // is unchanged from before companions existed.
        let config = companion_config("");
        let model = &config.local_models()[0];
        let temp = tempfile::TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(temp.path()).expect("store");
        let mut options = launch_options(model, 1);
        let before = options.clone();
        provision_companions(&store, model, &mut options).expect("no companions");
        assert_eq!(options, before);
        assert!(options.speculative.is_none());
        assert!(options.multimodal_projector.is_none());
    }
}
