//! The startup and artifact-provisioning flows behind the [`LocalRuntime`] entry points.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gateway_config::{Config, ModelKind};
use gateway_progress::Activity;
use gateway_routing::{Endpoint, Model, dominion_queues};
use tokio_util::sync::CancellationToken;

use super::hf_sidecar::maybe_write_sidecar;
use super::launch::{
    launch_options_for, provision_companion_paths, provision_companions, resolve_admission,
    serve_mode_for,
};
use super::{LocalRuntime, LocalStartFailure, LocalStartOutcome, StartPolicy, resolve_cache_root};
use crate::artifacts::{ArtifactStore, ProvisionedServer, ServerSelection};
use crate::dialect::resolve_local_dialect;
use crate::error::LocalError;
use crate::server::{LaunchOptions, ServerGuard};
use crate::upstream::LocalUpstream;

/// Shared body of [`LocalRuntime::provision_artifacts_with_cancellation`]
/// with the pinned-server provision injectable, so a test can drive it over
/// a mock layout the same way [`start_impl`] is driven.
pub(super) fn provision_artifacts_impl(
    config: &Config,
    progress: Option<&Activity>,
    token: &CancellationToken,
    provision: impl FnOnce(
        &ArtifactStore,
        &ServerSelection<'_>,
        Option<&Activity>,
    ) -> Result<ProvisionedServer, LocalError>,
) -> Result<Vec<LocalStartFailure>, LocalError> {
    if config.local_models().is_empty() {
        return Ok(Vec::new());
    }
    if token.is_cancelled() {
        return Err(LocalError::Cancelled);
    }
    // Kind preflight (A6): every model's kind is checked before the shared
    // server or any model provisions, so a profile with no launchable model
    // fails without a single side effect.
    let mut failures = Vec::new();
    let mut launchable = Vec::new();
    for local_model in config.local_models() {
        match serve_mode_for(local_model.kind()) {
            Ok(_) => launchable.push(local_model),
            Err(error) => failures.push(LocalStartFailure {
                model: local_model.name().to_owned(),
                error,
            }),
        }
    }
    if launchable.is_empty() {
        return Ok(failures);
    }
    let (store, _server) = provision_server(config, progress, provision)?;
    for local_model in launchable {
        // Phase boundary: a cancelled command provisions no further models.
        if token.is_cancelled() {
            return Err(LocalError::Cancelled);
        }
        let provisioned = store
            .ensure_model_with_cancellation(
                local_model.source(),
                local_model.sha256(),
                progress,
                Some(token),
            )
            .and_then(|_path| provision_companion_paths(&store, local_model, Some(token)));
        match provisioned {
            Ok(_companions) => {}
            // Cancellation is the command stopping, never a per-model fault.
            Err(LocalError::Cancelled) => return Err(LocalError::Cancelled),
            Err(error) => failures.push(LocalStartFailure {
                model: local_model.name().to_owned(),
                error,
            }),
        }
    }
    Ok(failures)
}

/// Resolves the cache root, builds the store, and provisions the pinned
/// `llama-server` per the `[local]` selection (explicit path, environment
/// variable, or the managed backend download).
fn provision_server(
    config: &Config,
    progress: Option<&Activity>,
    provision: impl FnOnce(
        &ArtifactStore,
        &ServerSelection<'_>,
        Option<&Activity>,
    ) -> Result<ProvisionedServer, LocalError>,
) -> Result<(ArtifactStore, ProvisionedServer), LocalError> {
    let cache_root = resolve_cache_root(config.local().cache_dir())?;
    tracing::info!(path = %cache_root.display(), "local model cache");
    let store = ArtifactStore::new(cache_root)?;
    let selection = ServerSelection {
        server_path: config.local().llama_server_path(),
        backend: config.local().llama_backend(),
    };
    if let Some(activity) = progress {
        activity.set_text("Provisioning llama-server");
    }
    let server = provision(&store, &selection, progress)?;
    tracing::info!(path = %server.executable.display(), "provisioned llama-server");
    Ok((store, server))
}

/// Shared body of [`LocalRuntime::start`] with the two externalities - the
/// pinned-server provision and the child spawn - injectable, so a test can
/// drive a start over a mock layout. `interrupted` is the readiness poll's
/// stop flag; `token`, when present, is checked at download chunk boundaries
/// and phase boundaries.
#[expect(
    clippy::too_many_lines,
    reason = "the per-model startup loop is one provisioning flow; splitting it would scatter the phase-boundary token checks across call sites"
)]
pub(super) fn start_impl(
    config: &Config,
    progress: Option<&Activity>,
    interrupted: &Arc<AtomicBool>,
    token: Option<&CancellationToken>,
    provision: impl FnOnce(
        &ArtifactStore,
        &ServerSelection<'_>,
        Option<&Activity>,
    ) -> Result<ProvisionedServer, LocalError>,
    spawn: impl Fn(&Path, &Path, &LaunchOptions, &AtomicBool) -> Result<ServerGuard, LocalError>,
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

    // A token already fired starts nothing at all.
    if token.is_some_and(CancellationToken::is_cancelled) {
        return Err(LocalError::Cancelled);
    }

    // Kind preflight (A6): every model's kind is checked before the shared
    // server provisions, so a profile with no launchable model fails without
    // a single provisioning side effect.
    let mut launchable = Vec::new();
    let mut failures = Vec::new();
    for local_model in config.local_models() {
        match serve_mode_for(local_model.kind()) {
            Ok(_) => launchable.push(local_model),
            Err(error) if policy == StartPolicy::FailFast => return Err(error),
            Err(error) => failures.push(LocalStartFailure {
                model: local_model.name().to_owned(),
                error,
            }),
        }
    }
    if launchable.is_empty() {
        return Ok(LocalStartOutcome {
            runtime: LocalRuntime {
                models: Vec::new(),
                upstreams: Vec::new(),
                cache_dir,
            },
            failures,
        });
    }
    let (store, server) = provision_server(config, progress, provision)?;

    let dominion_queues = dominion_queues(config);
    let mut started_models = Vec::with_capacity(launchable.len());

    for local_model in launchable {
        // Phase boundary: a cancelled command starts no further models, and
        // the models already started drop with this in-progress outcome,
        // killing their children.
        if token.is_some_and(CancellationToken::is_cancelled) {
            return Err(LocalError::Cancelled);
        }
        let started = (|| {
            let model_path = store.ensure_model_with_cancellation(
                local_model.source(),
                local_model.sha256(),
                progress,
                token,
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
            provision_companions(&store, local_model, &mut options, token)?;
            // Phase boundary: spawn only for an uncancelled command.
            if token.is_some_and(CancellationToken::is_cancelled) {
                return Err(LocalError::Cancelled);
            }
            if let Some(activity) = progress {
                activity.set_text(format!("Starting {}", local_model.name()));
            }
            let guard = spawn(
                &server.executable,
                &model_path,
                &options,
                interrupted.as_ref(),
            )?;
            let endpoint_id = format!("local-{}", local_model.name());
            // A non-chat child has no chat completions to dialect-match:
            // like a remote model, it takes the OpenAI default rather than
            // hard-failing on template-less `/props` evidence.
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

pub(super) fn retain_start<T>(
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
    // Cancellation is fatal in both policies: the command is stopping, so
    // later models must not start behind it.
    if policy == StartPolicy::FailFast || matches!(error, LocalError::Cancelled) {
        return Err(error);
    }
    failures.push(LocalStartFailure {
        model: model.to_owned(),
        error,
    });
    Ok(())
}
