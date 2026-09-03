//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](gateway_routing::Model).
//! Dropping [`LocalRuntime`] kills the children.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use gateway_config::{Config, LocalModelConfig, ModelKind, QueuePolicy, ThinkingMode};
use gateway_protocol::ShutdownError;
use gateway_routing::queue::DominionQueue;
use gateway_routing::{Endpoint, Model, dominion_queues};
use promptforge_progress::ProgressHandle;

use crate::artifacts::{self, ArtifactStore, ProvisionedServer, ServerSelection};
use crate::dialect::resolve_local_dialect;
use crate::error::LocalError;
use crate::launch_templates::resolve_chat_template_file;
use crate::server::{LaunchOptions, ServeMode, ServerGuard, SpeculativeLaunch};
use crate::sidecar;
use crate::upstream::LocalUpstream;

/// Running local `llama-server` children and the models they back.
///
/// Keep this value alive for the lifetime of the gateway process. Dropping it
/// terminates every child (via `LocalUpstream` Drop → `ServerGuard` Drop).
#[derive(Debug)]
pub struct LocalRuntime {
    models: Vec<Arc<Model>>,
    /// The upstreams behind `models`, kept un-erased so diagnostics can reach
    /// each child's captured output.
    upstreams: Vec<LocalUpstream>,
    /// The profile's `[local].cache_dir`, retained so the `/v1/cache` routes
    /// resolve the same root provisioning does, even with no local models.
    cache_dir: Option<String>,
}

/// Result of a best-effort local-model startup.
///
/// Successfully started children remain owned by [`runtime`](Self::runtime)
/// when another configured model fails to start.
///
/// # Examples
/// ```
/// use gateway_config::Config;
/// use gateway_local::LocalRuntime;
///
/// let config = Config::from_toml_str(
///     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
/// )?;
/// let outcome = LocalRuntime::start_partial(&config, None)?;
/// assert!(outcome.failures().is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalStartOutcome {
    runtime: LocalRuntime,
    failures: Vec<LocalStartFailure>,
}

impl LocalStartOutcome {
    /// Returns the successfully started local runtime.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # use gateway_local::LocalRuntime;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// # )?;
    /// let outcome = LocalRuntime::start_partial(&config, None)?;
    /// assert_eq!(outcome.runtime().child_count(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn runtime(&self) -> &LocalRuntime {
        &self.runtime
    }

    /// Returns one failure for each local model that did not start.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # use gateway_local::LocalRuntime;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// # )?;
    /// let outcome = LocalRuntime::start_partial(&config, None)?;
    /// assert!(outcome.failures().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn failures(&self) -> &[LocalStartFailure] {
        &self.failures
    }

    /// Splits the outcome into its running children and failures.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # use gateway_local::LocalRuntime;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// # )?;
    /// let outcome = LocalRuntime::start_partial(&config, None)?;
    /// let (runtime, failures) = outcome.into_parts();
    /// assert_eq!(runtime.child_count(), 0);
    /// assert!(failures.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn into_parts(self) -> (LocalRuntime, Vec<LocalStartFailure>) {
        (self.runtime, self.failures)
    }
}

/// One local model that failed during best-effort startup.
///
/// Values are reported by [`LocalRuntime::start_partial`].
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalStartFailure {
    model: String,
    error: LocalError,
}

impl LocalStartFailure {
    /// Returns the configured model name.
    ///
    /// # Examples
    /// ```no_run
    /// # use gateway_config::Config;
    /// # use gateway_local::LocalRuntime;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// # )?;
    /// for failure in LocalRuntime::start_partial(&config, None)?.failures() {
    ///     eprintln!("{} did not start", failure.model());
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the startup failure.
    ///
    /// # Examples
    /// ```no_run
    /// # use gateway_config::Config;
    /// # use gateway_local::LocalRuntime;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// # )?;
    /// for failure in LocalRuntime::start_partial(&config, None)?.failures() {
    ///     eprintln!("{}", failure.error());
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn error(&self) -> &LocalError {
        &self.error
    }
}

impl LocalRuntime {
    /// An empty runtime with no children. Used when no `[[local_model]]` is set
    /// and as the placeholder before the first profile switch.
    #[must_use]
    pub fn empty() -> LocalRuntime {
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
    /// `progress` is the parent leaf for startup provisioning, when the caller
    /// runs an operation tree: the pinned server stages once under a
    /// `llama-server` child (download/verify/extract leaves), and each local
    /// model gets its own subtree (download/verify leaves from provisioning
    /// plus an indeterminate `ready` leaf the spawn poll completes).
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub fn start(
        config: &Config,
        progress: Option<&ProgressHandle>,
    ) -> Result<LocalRuntime, LocalError> {
        let outcome = start_impl(
            config,
            progress,
            ArtifactStore::provision_llama_server_with_progress,
            ServerGuard::start,
            StartPolicy::FailFast,
        )?;
        Ok(outcome.runtime)
    }

    /// Provisions local models independently and retains every ready child.
    ///
    /// A failure shared by the whole runtime, such as staging
    /// `llama-server`, still returns immediately because no model can start.
    /// Per-model download, launch, readiness, and dialect failures are
    /// collected while later models continue.
    ///
    /// # Errors
    /// Returns [`LocalError`] when shared runtime provisioning fails.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::Config;
    /// use gateway_local::LocalRuntime;
    ///
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test\"\n",
    /// )?;
    /// let outcome = LocalRuntime::start_partial(&config, None)?;
    /// assert_eq!(outcome.runtime().child_count(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start_partial(
        config: &Config,
        progress: Option<&ProgressHandle>,
    ) -> Result<LocalStartOutcome, LocalError> {
        start_impl(
            config,
            progress,
            ArtifactStore::provision_llama_server_with_progress,
            ServerGuard::start,
            StartPolicy::KeepReady,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartPolicy {
    FailFast,
    KeepReady,
}

/// Resolves the cache root, builds the store, and provisions the pinned
/// `llama-server` per the `[local]` selection (explicit path, environment
/// variable, or the managed backend download).
fn provision_server(
    config: &Config,
    progress: Option<&ProgressHandle>,
    provision: impl FnOnce(
        &ArtifactStore,
        &ServerSelection<'_>,
        Option<&ProgressHandle>,
    ) -> Result<ProvisionedServer, LocalError>,
) -> Result<(ArtifactStore, ProvisionedServer), LocalError> {
    let cache_root = resolve_cache_root(config.local().cache_dir())?;
    tracing::info!(path = %cache_root.display(), "local model cache");
    let store = ArtifactStore::new(cache_root)?;
    let selection = ServerSelection {
        server_path: config.local().llama_server_path(),
        backend: config.local().llama_backend(),
    };
    let server_tree = progress.map(|handle| handle.child("llama-server", 1.0));
    let server = provision(&store, &selection, server_tree.as_ref())?;
    tracing::info!(path = %server.executable.display(), "provisioned llama-server");
    Ok((store, server))
}

/// Shared body of [`LocalRuntime::start`] with the two externalities - the
/// pinned-server provision and the child spawn - injectable, so a test can
/// drive a start over a mock layout.
fn start_impl(
    config: &Config,
    progress: Option<&ProgressHandle>,
    provision: impl FnOnce(
        &ArtifactStore,
        &ServerSelection<'_>,
        Option<&ProgressHandle>,
    ) -> Result<ProvisionedServer, LocalError>,
    spawn: impl Fn(
        &Path,
        &Path,
        &LaunchOptions,
        &AtomicBool,
        Option<&ProgressHandle>,
    ) -> Result<ServerGuard, LocalError>,
    policy: StartPolicy,
) -> Result<LocalStartOutcome, LocalError> {
    let cache_dir = config.local().cache_dir().map(str::to_owned);
    if config.local_models().is_empty() {
        return Ok(LocalStartOutcome {
            runtime: LocalRuntime {
                models: Vec::new(),
                upstreams: Vec::new(),
                cache_dir,
            },
            failures: Vec::new(),
        });
    }

    let (store, server) = provision_server(config, progress, provision)?;

    let interrupted = startup_interrupt_flag();
    let dominion_queues = dominion_queues(config);
    let mut started_models = Vec::with_capacity(config.local_models().len());
    let mut failures = Vec::new();

    for local_model in config.local_models() {
        let model_tree = progress.map(|handle| handle.child(local_model.name(), 3.0));
        let started = (|| {
            let model_path = store.ensure_model_with_progress(
                local_model.source(),
                local_model.sha256(),
                model_tree.as_ref(),
            )?;
            tracing::info!(
                model = %local_model.name(),
                path = %model_path.display(),
                "provisioned local GGUF"
            );

            maybe_write_sidecar(&store, local_model.source(), &model_path);

            let admission = resolve_admission(&dominion_queues, local_model)?;
            let mut options = launch_options_for(&store, local_model, &model_path, &admission)?;
            options.path_prefix.clone_from(&server.path_prefix);
            provision_companions(&store, local_model, &mut options)?;
            let ready = model_tree.as_ref().map(|tree| tree.child("ready", 2.0));
            let guard = spawn(
                &server.executable,
                &model_path,
                &options,
                interrupted.as_ref(),
                ready.as_ref(),
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
                model_path,
                options,
                local_model.name().to_owned(),
            );
            let model = Arc::new(Model {
                name: local_model.name().to_owned(),
                kind: local_model.kind(),
                description: local_model.description().to_owned(),
                context: local_model.context(),
                thinking: local_model.thinking(),
                capabilities: local_model.capabilities().clone(),
                tool_dialect: tool_dialect.to_owned(),
                upstream_name,
                endpoint: Arc::new(Endpoint {
                    id: endpoint_id,
                    upstream: Arc::new(upstream.clone()),
                    queue: admission.queue,
                }),
            });
            tracing::info!(
                model = %local_model.name(),
                base_url = %base_url,
                "local llama-server ready"
            );
            Ok::<_, LocalError>((model, upstream))
        })();
        retain_start(
            policy,
            local_model.name(),
            started,
            &mut started_models,
            &mut failures,
        )?;
    }
    let (models, upstreams) = started_models.into_iter().unzip();

    Ok(LocalStartOutcome {
        runtime: LocalRuntime {
            models,
            upstreams,
            cache_dir,
        },
        failures,
    })
}

fn retain_start<T>(
    policy: StartPolicy,
    model: &str,
    result: Result<T, LocalError>,
    started: &mut Vec<T>,
    failures: &mut Vec<LocalStartFailure>,
) -> Result<(), LocalError> {
    let error = match result {
        Ok(value) => {
            started.push(value);
            return Ok(());
        }
        Err(error) => error,
    };
    if policy == StartPolicy::FailFast {
        return Err(error);
    }
    failures.push(LocalStartFailure {
        model: model.to_owned(),
        error,
    });
    Ok(())
}

impl LocalRuntime {
    /// Bounded captured-output tails of the running local children, keyed by
    /// configured model name.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<(String, String)> {
        self.upstreams
            .iter()
            .map(|upstream| (upstream.model_name().to_owned(), upstream.diagnostics()))
            .collect()
    }

    /// Models registered for local inference, in `[[local_model]]` order.
    #[must_use]
    pub fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    /// The profile's configured `[local].cache_dir`, when set.
    #[must_use]
    pub fn cache_dir(&self) -> Option<&str> {
        self.cache_dir.as_deref()
    }

    /// Number of local model endpoints (each owns one `llama-server` child).
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.models.len()
    }

    /// Explicitly terminate every owned `llama-server` child and disable respawn,
    /// returning the first teardown failure after attempting *all* children.
    ///
    /// Dropping the runtime does not guarantee child termination, because the
    /// routing table holds `Arc<dyn Upstream>` clones of these same models, so
    /// the runtime is not the sole owner (PFGL-MOD-001). This drives an explicit
    /// teardown through the [`Upstream`](gateway_protocol::upstream::Upstream) seam so a
    /// profile switch frees the old children's VRAM deterministically before the
    /// replacement profile's children start. Every child is torn down even if an
    /// earlier one fails, so one stuck child never strands the rest.
    ///
    /// # Errors
    /// Returns the first [`ShutdownError`] a child teardown produced.
    pub fn shutdown(&self) -> Result<(), ShutdownError> {
        let mut first_error: Option<ShutdownError> = None;
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
pub fn resolve_cache_root(configured: Option<&str>) -> Result<PathBuf, LocalError> {
    match configured {
        Some(path) if !path.is_empty() => artifacts::expand_tilde(path),
        // An unset cache_dir defaults to `~/.promptforge`; a missing home is a
        // typed error rather than a silent working-directory fallback (ART-009).
        _ => artifacts::default_promptforge_root_checked(),
    }
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
        chat_template_file: None,
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

fn launch_options_for(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    model_path: &Path,
    admission: &LocalAdmission,
) -> Result<LaunchOptions, LocalError> {
    let mut options = launch_options(model, admission.parallel);
    options.chat_template_file = resolve_chat_template_file(store, model, model_path)?;
    Ok(options)
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
            Ok(Some(meta)) if sidecar_is_current(&meta, source) => {
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
        .user_agent(concat!("gateway/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "could not build sidecar HTTP client");
            write_sidecar_metadata(source, model_path, None, None);
            return;
        }
    };

    let bearer = artifacts::hub_bearer_token_from_env();
    let (chat_template, fetched) =
        match sidecar::fetch_hf_chat_template(&client, source, bearer.as_deref()) {
            Ok(Some(template)) => (Some(template), Some(sidecar::utc_now_iso())),
            Ok(None) => {
                tracing::debug!(source = %source, "no chat_template from HF metadata");
                (None, Some(sidecar::utc_now_iso()))
            }
            Err(error) => {
                // Deliberate downgrade (SIDECAR-006): the sidecar is supplementary,
                // so a fetch failure is logged and skipped, not propagated. Source
                // provenance is still persisted for conservative model-ID matching.
                tracing::debug!(source = %source, error = %error, "sidecar fetch failed");
                (None, None)
            }
        };
    write_sidecar_metadata(source, model_path, fetched, chat_template);
}

fn sidecar_is_current(metadata: &sidecar::SidecarMeta, source: &str) -> bool {
    metadata.chat_template.is_some() && metadata.source.as_deref() == Some(source)
}

fn write_sidecar_metadata(
    source: &str,
    model_path: &Path,
    fetched: Option<String>,
    chat_template: Option<String>,
) {
    let meta = sidecar::SidecarMeta {
        source: Some(source.to_owned()),
        fetched,
        chat_template,
        card: None,
    };
    let sidecar_file = sidecar::sidecar_path(model_path);
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
    use gateway_config::Config;

    #[test]
    fn sidecar_keeps_model_id_provenance_when_remote_template_is_unavailable() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let model_path = temp.path().join("model.gguf");
        let source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/model.gguf";

        write_sidecar_metadata(source, &model_path, None, None);

        let metadata = sidecar::read_sidecar(&model_path)
            .expect("read provenance sidecar")
            .expect("sidecar exists");
        assert_eq!(
            metadata.source_model_id().as_deref(),
            Some("unsloth/gemma-4-E2B-it-GGUF")
        );
        assert!(metadata.fetched.is_none());
        assert!(metadata.chat_template.is_none());
    }

    #[test]
    fn sidecar_cache_hit_requires_template_and_matching_source_provenance() {
        let source = "https://huggingface.co/org/model/resolve/main/model.gguf";
        let mut metadata = sidecar::SidecarMeta {
            source: Some(source.to_owned()),
            fetched: None,
            chat_template: Some("{{ messages }}".to_owned()),
            card: None,
        };
        assert!(sidecar_is_current(&metadata, source));

        metadata.source =
            Some("https://huggingface.co/other/model/resolve/main/model.gguf".to_owned());
        assert!(!sidecar_is_current(&metadata, source));
        metadata.source = Some(source.to_owned());
        metadata.chat_template = None;
        assert!(!sidecar_is_current(&metadata, source));
    }

    #[test]
    fn empty_local_models_starts_noop_runtime() {
        let config = Config::from_toml_str(
            r#"
config-version = 2

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
        let runtime = LocalRuntime::start(&config, None).expect("empty local runtime");
        assert_eq!(runtime.child_count(), 0);
        assert!(runtime.models().is_empty());
        assert!(runtime.diagnostics().is_empty());
    }

    #[test]
    fn partial_policy_retains_successes_and_collects_each_failure() {
        let mut started = Vec::new();
        let mut failures = Vec::new();
        retain_start(
            StartPolicy::KeepReady,
            "ready",
            Ok(7),
            &mut started,
            &mut failures,
        )
        .expect("partial startup keeps ready models");
        retain_start(
            StartPolicy::KeepReady,
            "first",
            Err(LocalError::EarlyExit {
                status: "first stopped".to_owned(),
            }),
            &mut started,
            &mut failures,
        )
        .expect("partial startup continues");
        retain_start(
            StartPolicy::KeepReady,
            "second",
            Err(LocalError::EarlyExit {
                status: "second stopped".to_owned(),
            }),
            &mut started,
            &mut failures,
        )
        .expect("partial startup continues");

        assert_eq!(started, [7]);
        assert_eq!(
            failures
                .iter()
                .map(LocalStartFailure::model)
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn start_with_no_local_models_registers_no_leaves() {
        // An empty `[[local_model]]` set is a no-op start: the parent leaf
        // gains no children at all.
        let config = Config::from_toml_str(
            r#"
config-version = 2

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
        let hub = Arc::new(promptforge_progress::ProgressHub::new());
        let tree = hub.operation();
        let parent = tree.register("local-models", 1.0);
        let runtime = LocalRuntime::start(&config, Some(&parent)).expect("empty local runtime");
        assert_eq!(runtime.child_count(), 0);
        let snapshot = hub.snapshot();
        let paths: Vec<&str> = snapshot[0]
            .nodes
            .iter()
            .map(|node| node.path.as_str())
            .collect();
        assert_eq!(paths, ["local-models"]);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
    fn start_over_a_mock_layout_registers_the_expected_subtree_shape() {
        use crate::testsupport::hex_sha256;

        // The mock layout provisions for real - a path source with a true pin
        // - but has no `llama-server` binary to spawn, so the start fails at
        // launch, after the subtree shape is registered.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let model_file = temp.path().join("mock.gguf");
        std::fs::write(&model_file, b"mock-gguf-bytes").expect("write model");
        let config = Config::from_toml_str(&format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
cache_dir = '{}'

[[local_model]]
name = "mock"
kind = "embedding"
description = "a mock local model"
source = '{}'
sha256 = "{}"
context = 512
"#,
            temp.path().join("cache").display(),
            model_file.display(),
            hex_sha256(b"mock-gguf-bytes"),
        ))
        .expect("config");

        let hub = Arc::new(promptforge_progress::ProgressHub::new());
        let tree = hub.operation();
        let parent = tree.register("local-models", 1.0);

        let error = start_impl(
            &config,
            Some(&parent),
            |_store, _selection, server| {
                // An already-staged server has no download/verify/extract work.
                if let Some(handle) = server {
                    handle.complete();
                }
                Ok(ProvisionedServer {
                    executable: PathBuf::from("mock-llama-server"),
                    path_prefix: Vec::new(),
                })
            },
            |_, _, _, _, _| {
                Err(LocalError::EarlyExit {
                    status: "the mock layout has no llama-server to spawn".to_owned(),
                })
            },
            StartPolicy::FailFast,
        )
        .expect_err("the mock layout cannot launch a real child");
        assert!(matches!(error, LocalError::EarlyExit { .. }));

        let snapshot = hub.snapshot();
        assert_eq!(snapshot.len(), 1);
        let nodes = &snapshot[0].nodes;
        let paths: Vec<&str> = nodes.iter().map(|node| node.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "local-models",
                "local-models/llama-server",
                "local-models/mock",
                "local-models/mock/download",
                "local-models/mock/verify",
                "local-models/mock/ready",
            ]
        );
        // The path source needed no download and the pin ran a real hash
        // pass; the readiness poll never ran.
        assert_eq!(nodes[3].fraction, 1.0);
        assert_eq!(nodes[4].fraction, 1.0);
        assert_eq!(nodes[5].fraction, 0.0);
    }

    #[tokio::test]
    async fn parallel_field_feeds_parallel_arg_and_queue_limit() {
        // A local model with `parallel = 3` launches its child with
        // `--parallel 3` (launch_options carries the number; the server tests
        // prove it renders into the argv) and admits at most 3 concurrent
        // requests through its per-model queue.
        let config = Config::from_toml_str(
            r#"
config-version = 2

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
config-version = 2

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
config-version = 2

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
config-version = 2

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
config-version = 2

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
