//! Gateway-owned local generative inference via a managed `llama-server` child.
//!
//! In-process `llama-cpp-2` linking is deferred. Layer 2 provisions a pinned
//! `llama-server` binary, downloads each configured GGUF into the operator
//! cache, spawns one child per `[[local_model]]`, and registers each as a
//! normal OpenAI-routed [`Model`](gateway_routing::Model).
//! Dropping [`LocalRuntime`] kills the children.

mod hf_sidecar;
mod launch;
mod start;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;

use gateway_config::Config;
use gateway_progress::Activity;
use gateway_protocol::ShutdownError;
use gateway_routing::Model;
use gateway_routing::queue::DominionQueue;
use tokio_util::sync::CancellationToken;

use crate::artifacts::{self, ArtifactStore};
use crate::error::LocalError;
use crate::server::{ServerGuard, SpeculativeLaunch};
use crate::upstream::LocalUpstream;
use start::{provision_artifacts_impl, start_impl};

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
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalStartOutcome {
    runtime: LocalRuntime,
    failures: Vec<LocalStartFailure>,
}

impl LocalStartOutcome {
    /// Returns the successfully started local runtime.
    #[must_use]
    pub fn runtime(&self) -> &LocalRuntime {
        &self.runtime
    }

    /// Returns one failure for each local model that did not start.
    #[must_use]
    pub fn failures(&self) -> &[LocalStartFailure] {
        &self.failures
    }

    /// Splits the outcome into its running children and failures.
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
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the startup failure.
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

    /// Builds a child-free runtime around supplied model bindings.
    ///
    /// This fixture lets profile-switch tests prove routing replacement without
    /// downloading a model or launching `llama-server`.
    #[cfg(feature = "test-fixtures")]
    #[doc(hidden)]
    #[must_use]
    pub fn from_test_models(models: Vec<Arc<Model>>) -> LocalRuntime {
        LocalRuntime {
            models,
            upstreams: Vec::new(),
            cache_dir: None,
        }
    }

    /// Provisions binaries/models and starts one `llama-server` per local model.
    ///
    /// When the config declares no `[[local_model]]`, returns an empty runtime
    /// without downloading anything.
    ///
    /// `progress` is the caller's live activity, when it runs one: the
    /// pinned server's download, verify, and extract stages, each model's
    /// download and verify, and `"Starting {model}"` before each spawn are
    /// written into its text.
    ///
    /// # Errors
    /// Returns [`LocalError`] when download, verification, spawn, or readiness fails.
    pub fn start(config: &Config, progress: Option<&Activity>) -> Result<LocalRuntime, LocalError> {
        let outcome = start_impl(
            config,
            progress,
            &startup_interrupt_flag(),
            None,
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
    pub fn start_partial(
        config: &Config,
        progress: Option<&Activity>,
    ) -> Result<LocalStartOutcome, LocalError> {
        start_impl(
            config,
            progress,
            &startup_interrupt_flag(),
            None,
            ArtifactStore::provision_llama_server_with_progress,
            ServerGuard::start,
            StartPolicy::KeepReady,
        )
    }

    /// [`Self::start_partial`] variant that stops promptly when `token`
    /// fires: downloads stop at the next chunk boundary, phase boundaries
    /// (verify, extract, spawn) check the token, and a cancellation is
    /// [`LocalError::Cancelled`] rather than a partial outcome. Children
    /// already started are dropped with the in-progress outcome, so their
    /// processes die with it.
    ///
    /// `interrupted` is the child-readiness poll's cancellation flag, which
    /// predates the token and speaks `AtomicBool`: the async caller bridges
    /// the token onto it so one cancellation source stops a child that is
    /// still loading weights. This function is synchronous and CPU-quiet but
    /// blocking on I/O; async callers run it on `spawn_blocking`.
    ///
    /// # Errors
    /// Returns [`LocalError`] when shared runtime provisioning fails or the
    /// token fires.
    pub fn start_partial_with_cancellation(
        config: &Config,
        progress: Option<&Activity>,
        token: &CancellationToken,
        interrupted: &Arc<AtomicBool>,
    ) -> Result<LocalStartOutcome, LocalError> {
        start_impl(
            config,
            progress,
            interrupted,
            Some(token),
            |store, selection, server| {
                store.provision_llama_server_with_cancellation(selection, server, Some(token))
            },
            ServerGuard::start,
            StartPolicy::KeepReady,
        )
    }

    /// Provisions every artifact the configured local models need - the
    /// pinned `llama-server`, each model's GGUF, and any companion (a
    /// speculative drafter, a multimodal projector) - without spawning a
    /// child. A later [`Self::start_partial_with_cancellation`] over the
    /// same config then finds every blob in the cache and goes straight to
    /// the spawn, so a caller can keep its old children serving through the
    /// download and stop them only right before the new ones start.
    ///
    /// Per-model provisioning failures are collected and returned, not
    /// fatal: the start over the same config reports them again as its own
    /// per-model failures, so the models that did provision still start.
    /// `progress`, when given, receives the same `llama-server` and
    /// per-model download and verify text the start would write.
    ///
    /// # Errors
    /// Returns [`LocalError`] when the shared `llama-server` provisioning
    /// fails, or [`LocalError::Cancelled`] when `token` fires - checked
    /// before the first request, at download chunk boundaries, and between
    /// models.
    pub fn provision_artifacts_with_cancellation(
        config: &Config,
        progress: Option<&Activity>,
        token: &CancellationToken,
    ) -> Result<Vec<LocalStartFailure>, LocalError> {
        provision_artifacts_impl(config, progress, token, |store, selection, server| {
            store.provision_llama_server_with_cancellation(selection, server, Some(token))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartPolicy {
    FailFast,
    KeepReady,
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

    /// Removes one started model and its upstream from the runtime, returning
    /// the model so the caller can tear the child down through the
    /// [`Upstream`](gateway_protocol::upstream::Upstream) seam (which disables
    /// respawn before killing the process). Returns `None` when no started
    /// model is named `name`.
    ///
    /// The caller owns the teardown: `shutdown` on the returned model's
    /// endpoint upstream blocks on the child's exit, so async callers run it
    /// on `spawn_blocking`.
    #[must_use]
    pub fn unload_model(&mut self, name: &str) -> Option<Arc<Model>> {
        let index = self.models.iter().position(|model| model.name == name)?;
        let model = self.models.remove(index);
        // The upstreams vector is parallel to `models`; dropping this handle
        // leaves the endpoint's `Arc<dyn Upstream>` clone alive for the
        // caller's teardown.
        drop(self.upstreams.remove(index));
        Some(model)
    }

    /// Explicitly terminates every owned `llama-server` child and disables respawn,
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

/// The owned cache paths of one model's provisioned companions; `None` for
/// a companion the model does not declare.
#[derive(Debug, Default)]
struct CompanionPaths {
    speculative: Option<SpeculativeLaunch>,
    multimodal_projector: Option<PathBuf>,
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
mod tests;
