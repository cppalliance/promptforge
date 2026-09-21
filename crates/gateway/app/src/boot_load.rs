//! The boot load: the one queue command that fills the local runtime.
//!
//! The runner publishes the remote routing table at assembly and, when a
//! profile is selected, enqueues one `LoadProfile`. Its body runs here in
//! order: prepare the profile's local and speech-to-text members
//! (`loading-profile`), download their artifacts (`downloading-models`),
//! publish the local names as [`LiveState::loading`](crate::LiveState),
//! spawn the children under one deadline (`starting-models`), commit the
//! ready ones into the live routing table under one write, then make the
//! process's one guarded STT load (`loading-speech`). Nothing after this
//! command changes the local runtime: a later profile change persists and
//! reports `restart_required`, and an apply swaps only the remote table.
//!
//! A failure before the commit leaves the remote table serving and clears
//! `loading`, so a local model whose spawn failed answers a plain 404 rather
//! than a lingering 503. There is nothing to roll back: the live state
//! before the commit is the state the runner assembled.

use std::sync::Arc;
#[cfg(feature = "local")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "local")]
use std::time::Duration;

use gateway_config::{Config, ProfileName};
use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::error::GatewayError;
#[cfg(feature = "local")]
use crate::local::LocalRuntime;

/// Deadline for spawning every local child of the selected profile.
#[cfg(feature = "local")]
const SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Error message when a configuration declaring `[[local_model]]` reaches a
/// build compiled without the `local` feature.
#[cfg(not(feature = "local"))]
pub(crate) const LOCAL_MODELS_UNSUPPORTED: &str =
    "configuration declares [[local_model]] but this build lacks the `local` feature";

/// Error when STT reaches a gateway build without the heavy runtime.
#[cfg(not(feature = "stt"))]
pub(crate) const STT_RUNTIME_UNAVAILABLE: &str =
    "the active profile selects [[stt_model]] but this build lacks the `stt` feature";

/// The `LoadProfile` body: loads `name`'s local members into the live
/// runtime, then makes the process's one guarded STT load.
///
/// The command's activity is shared with the blocking stages, each of
/// which writes its own text (`"Loading profile"`, the artifact store's
/// download and verify lines, `"Starting {model}"`, the speech load's
/// stages); the guard drops with this function's return on every path.
///
/// Returns the profile name on success. A partial start (some children
/// ready, others failed) commits the ready children and reports the rest
/// through [`GatewayError::PartialStart`]; only a fully or partially
/// published profile proceeds to the STT load, whose failure fails the
/// command but never the gateway. Any failure under a fired `token`
/// reports as [`GatewayError::CommandCancelled`], however deep the stop
/// landed.
pub(crate) async fn run(
    state: &AppState,
    name: ProfileName,
    activity: Activity,
    token: &CancellationToken,
) -> Result<String, GatewayError> {
    let label = format!("load-profile: {name}");
    let activity = Arc::new(activity);
    let result = match load_local(state, &name, &activity, token).await {
        Ok(()) => Ok(name.to_string()),
        Err(_) if token.is_cancelled() => Err(GatewayError::CommandCancelled(label.clone())),
        Err(error) => Err(error),
    };
    #[cfg(feature = "stt")]
    {
        let published = match &result {
            Ok(_) => true,
            #[cfg(feature = "local")]
            Err(GatewayError::PartialStart { .. }) => true,
            Err(_) => false,
        };
        if published {
            load_speech(state, &activity, token, &label).await?;
        } else {
            tracing::warn!(
                profile = %name,
                "the local load did not publish; the speech load is skipped"
            );
        }
    }
    #[cfg(not(feature = "stt"))]
    let _ = label;
    result
}

/// The local half: prepare, download, publish `loading`, spawn, commit.
async fn load_local(
    state: &AppState,
    name: &ProfileName,
    activity: &Arc<Activity>,
    token: &CancellationToken,
) -> Result<(), GatewayError> {
    if token.is_cancelled() {
        return Err(cancelled(name));
    }
    let config = prepare(state, name, activity).await?;
    #[cfg(not(feature = "local"))]
    {
        // `prepare` refused any local member, so there is nothing to load.
        let _ = config;
        Ok(())
    }
    #[cfg(feature = "local")]
    {
        if config.local_models().is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        state.park_at(crate::park::Phase::Download).await;
        download_artifacts(&config, activity, token).await?;
        if token.is_cancelled() {
            return Err(cancelled(name));
        }
        publish_loading(state, &config).await;
        #[cfg(test)]
        state.park_at(crate::park::Phase::Spawn).await;
        let (runtime, failures) = match spawn_children(&config, activity, token).await {
            Ok(outcome) => outcome,
            Err(error) => {
                state.live.write().await.loading.clear();
                return Err(error);
            }
        };
        commit(state, name, runtime, failures).await
    }
}

/// Resolves the profile's members from the live catalog under the
/// `"Loading profile"` text, refusing members this build cannot run.
async fn prepare(
    state: &AppState,
    name: &ProfileName,
    activity: &Activity,
) -> Result<Config, GatewayError> {
    activity.set_text("Loading profile");
    tracing::info!(profile = %name, "loading profile");
    let catalog = Arc::clone(&state.live.read().await.config);
    if !catalog
        .profiles()
        .iter()
        .any(|profile| profile.name() == name.as_str())
    {
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }
    let config = catalog
        .select_profile(Some(name))
        .map_err(|error| GatewayError::switch_failed("select-profile", error))?;
    #[cfg(not(feature = "local"))]
    if !config.local_models().is_empty() {
        return Err(GatewayError::switch_failed(
            "start-local",
            std::io::Error::other(LOCAL_MODELS_UNSUPPORTED),
        ));
    }
    #[cfg(not(feature = "stt"))]
    if !config.stt_models().is_empty() {
        return Err(GatewayError::switch_failed(
            "start-stt",
            std::io::Error::other(STT_RUNTIME_UNAVAILABLE),
        ));
    }
    Ok(config)
}

fn cancelled(name: &ProfileName) -> GatewayError {
    GatewayError::CommandCancelled(format!("load-profile: {name}"))
}

/// Stages every artifact the local members need through the artifact
/// store, off the async executor, cancellable at chunk boundaries. A
/// per-model provisioning failure is logged and left for the spawn to
/// report; only a store-level failure fails the load here.
#[cfg(feature = "local")]
async fn download_artifacts(
    config: &Config,
    activity: &Arc<Activity>,
    token: &CancellationToken,
) -> Result<(), GatewayError> {
    activity.set_text("Downloading models");
    tracing::info!("downloading local model artifacts");
    let config = config.clone();
    let progress = Arc::clone(activity);
    let worker_token = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        LocalRuntime::provision_artifacts_with_cancellation(&config, Some(&progress), &worker_token)
    })
    .await;
    match result {
        Ok(Ok(failures)) => {
            for failure in &failures {
                tracing::warn!(
                    model = failure.model(),
                    error = %failure.error(),
                    "local model artifact did not provision; the start reports it"
                );
            }
            if failures.is_empty() {
                tracing::info!("local model artifacts are in the cache");
            }
            Ok(())
        }
        Ok(Err(error)) => Err(GatewayError::switch_failed("download-models", error)),
        Err(error) => Err(GatewayError::switch_failed("download-models-task", error)),
    }
}

/// Promises the profile's local models as loading, so a request for one
/// receives [`GatewayError::ModelLoading`] until the commit or the failure
/// withdraws the promise.
#[cfg(feature = "local")]
async fn publish_loading(state: &AppState, config: &Config) {
    let mut live = state.live.write().await;
    live.loading = config
        .local_models()
        .iter()
        .map(|model| model.name().to_owned())
        .collect();
}

/// Spawns every local child under [`SPAWN_TIMEOUT`] on the blocking pool.
/// Cancellation and the deadline both flip the start's interrupt flag, so
/// a start still running after either stops at its next checkpoint and
/// the children it did spawn stop with the dropped runtime.
#[cfg(feature = "local")]
async fn spawn_children(
    config: &Config,
    activity: &Arc<Activity>,
    token: &CancellationToken,
) -> Result<(LocalRuntime, Vec<crate::local::LocalStartFailure>), GatewayError> {
    activity.set_text("Starting models");
    tracing::info!("starting local models");
    let start_config = config.clone();
    let start_progress = Arc::clone(activity);
    let start_token = token.clone();
    let interrupted = Arc::new(AtomicBool::new(false));
    let worker_interrupted = Arc::clone(&interrupted);
    let bridge = tokio::spawn({
        let interrupted = Arc::clone(&interrupted);
        let token = token.clone();
        async move {
            token.cancelled().await;
            interrupted.store(true, Ordering::Release);
        }
    });
    let started = tokio::time::timeout(
        SPAWN_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            LocalRuntime::start_partial_with_cancellation(
                &start_config,
                Some(&start_progress),
                &start_token,
                &worker_interrupted,
            )
        }),
    )
    .await;
    bridge.abort();
    let outcome = match started {
        Ok(Ok(Ok(outcome))) => outcome,
        Ok(Ok(Err(error))) => return Err(GatewayError::switch_failed("start-local", error)),
        Ok(Err(join)) => return Err(GatewayError::switch_failed("start-local-task", join)),
        Err(_) => {
            interrupted.store(true, Ordering::Release);
            tracing::error!(
                deadline = ?SPAWN_TIMEOUT,
                "local model startup exceeded the boot deadline; interrupting it"
            );
            return Err(GatewayError::switch_failed(
                "start-local-timeout",
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "local model startup exceeded the boot deadline",
                ),
            ));
        }
    };
    let (runtime, failures) = outcome.into_parts();
    for failure in &failures {
        tracing::warn!(
            model = failure.model(),
            error = %failure.error(),
            "local model did not start"
        );
    }
    Ok((runtime, failures))
}

/// Merges the ready children into the live routing table, installs the
/// runtime, and withdraws the loading promise, all under one live write.
#[cfg(feature = "local")]
async fn commit(
    state: &AppState,
    name: &ProfileName,
    runtime: LocalRuntime,
    failures: Vec<crate::local::LocalStartFailure>,
) -> Result<(), GatewayError> {
    let loaded: Vec<String> = runtime
        .models()
        .iter()
        .map(|model| model.name.clone())
        .collect();
    {
        let mut live = state.live.write().await;
        live.loading.clear();
        let routing = live
            .routing
            .as_ref()
            .clone()
            .merge(runtime.models().iter().cloned())
            .map_err(|error| GatewayError::switch_failed("merge-routing", error))?;
        live.routing = Arc::new(routing);
        live.local = runtime;
    }
    if failures.is_empty() {
        tracing::info!(profile = %name, "loaded profile");
        return Ok(());
    }
    Err(GatewayError::PartialStart {
        profile: name.to_string(),
        loaded,
        failed: failures
            .iter()
            .map(|failure| format!("{}: {}", failure.model(), failure.error()))
            .collect(),
    })
}

/// The process's one guarded STT load, after the local half published. A
/// failure fails the boot command but never the gateway: speech stays
/// unavailable until the process restarts, and no later command retries.
#[cfg(feature = "stt")]
async fn load_speech(
    state: &AppState,
    activity: &Arc<Activity>,
    token: &CancellationToken,
    label: &str,
) -> Result<(), GatewayError> {
    activity.set_text("Loading speech");
    tracing::info!("loading the speech runtime");
    let service = state.speech.clone();
    let config = state.live.read().await.config.as_ref().clone();
    let progress = Arc::clone(activity);
    let worker_token = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        service.load_initial(&config, Some(&progress), &worker_token)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            tracing::info!("speech runtime loaded");
            Ok(())
        }
        Ok(Err(_)) if token.is_cancelled() => Err(GatewayError::CommandCancelled(label.to_owned())),
        Ok(Err(error)) => Err(GatewayError::switch_failed("load-speech", error)),
        Err(join) => Err(GatewayError::switch_failed("load-speech-task", join)),
    }
}

#[cfg(test)]
#[path = "boot_load-tests.rs"]
mod tests;
