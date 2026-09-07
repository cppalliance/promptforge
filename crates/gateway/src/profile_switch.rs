//! Private profile-switch preparation transaction.
//!
//! The prepared and cutover values own each phase's resources, so runtime
//! staging cannot begin before target preparation and interim publication.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use gateway_config::{Config, ProfileName};
use rand::Rng as _;
use shared_progress::{ProgressHandle, ProgressTree};
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::error::GatewayError;
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
#[cfg(feature = "web-search")]
use gateway_web_search::WebSearchState;

const PREPARED_CREATE_ATTEMPTS: u64 = 16;
/// Shared deadline for target staging and prior-runtime reconstruction.
pub(super) const STAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
static PERSISTENCE_NAMES: LazyLock<ProcessPreparationNames<fn() -> u128>> =
    LazyLock::new(|| ProcessPreparationNames::new(std::process::id(), random_persistence_nonce));

/// Error message when a configuration declaring `[[local_model]]` reaches a
/// build compiled without the `local` feature.
#[cfg(not(feature = "local"))]
pub(crate) const LOCAL_MODELS_UNSUPPORTED: &str =
    "configuration declares [[local_model]] but this build lacks the `local` feature";

/// Error when STT reaches a gateway build without the heavy runtime.
#[cfg(not(feature = "stt"))]
pub(crate) const STT_RUNTIME_UNAVAILABLE: &str =
    "the active profile selects [[stt_model]] but this build lacks the `stt` feature";

/// How a successful switch commits its active-profile state.
pub(crate) enum StatePersistence {
    /// The selection already matches persisted state.
    None,
    /// Atomically replace real state while preserving any pending shadow.
    Write,
    /// Promote the shadows an Apply captured: each capture's contents land in
    /// its real file, and the shadow is deleted only when it still holds
    /// those contents, so a save that raced the apply stays pending.
    Promote(Vec<crate::config_apply::ShadowCapture>),
}

struct ProcessPreparationNames<N> {
    pid: u32,
    nonce: OnceLock<u128>,
    sequence: AtomicU64,
    random_nonce: N,
}

impl<N: Fn() -> u128> ProcessPreparationNames<N> {
    fn new(pid: u32, random_nonce: N) -> Self {
        Self {
            pid,
            nonce: OnceLock::new(),
            sequence: AtomicU64::new(0),
            random_nonce,
        }
    }

    fn nonce(&self) -> u128 {
        *self.nonce.get_or_init(|| (self.random_nonce)())
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }
}

/// One fully written and synced temporary file awaiting atomic replacement.
#[derive(Debug)]
struct PreparedFile {
    target: PathBuf,
    temporary: Option<PathBuf>,
    original: Option<Vec<u8>>,
    contents: Vec<u8>,
}

impl PreparedFile {
    fn prepare(target: PathBuf, contents: String) -> Result<Self, GatewayError> {
        Self::prepare_with_name_source(target, contents, &PERSISTENCE_NAMES)
    }

    fn prepare_with_name_source<N: Fn() -> u128>(
        target: PathBuf,
        contents: String,
        names: &ProcessPreparationNames<N>,
    ) -> Result<Self, GatewayError> {
        Self::prepare_with_names(target, contents, names.pid, names.nonce(), || {
            names.next_sequence()
        })
    }

    fn prepare_with_names(
        target: PathBuf,
        contents: String,
        pid: u32,
        nonce: u128,
        mut next_sequence: impl FnMut() -> u64,
    ) -> Result<Self, GatewayError> {
        let original = match std::fs::read(&target) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(GatewayError::ConfigWriteIo(Box::new(error))),
        };
        let (mut file, temporary) = create_prepared(&target, pid, nonce, &mut next_sequence)
            .map_err(|error| GatewayError::ConfigWriteIo(Box::new(error)))?;
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(GatewayError::ConfigWriteIo(Box::new(error)));
        }
        Ok(Self {
            target,
            temporary: Some(temporary),
            original,
            contents: contents.into_bytes(),
        })
    }

    fn commit(&mut self) -> Result<(), std::io::Error> {
        let temporary = self.temporary.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "prepared persistence was already committed",
            )
        })?;
        std::fs::rename(temporary, &self.target)?;
        self.temporary = None;
        Ok(())
    }

    fn still_original(&self) -> bool {
        match (&self.original, std::fs::read(&self.target)) {
            (Some(original), Ok(current)) => &current == original,
            (None, Err(error)) => error.kind() == std::io::ErrorKind::NotFound,
            _ => false,
        }
    }

    fn has_committed_contents(&self) -> bool {
        std::fs::read(&self.target).is_ok_and(|current| current == self.contents)
    }

    fn target(&self) -> &Path {
        &self.target
    }

    #[cfg(test)]
    fn discard_temporary(&self) {
        let temporary = self
            .temporary
            .as_ref()
            .expect("uncommitted preparation owns a temporary");
        std::fs::remove_file(temporary).expect("prepared temporary exists");
    }
}

impl Drop for PreparedFile {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = std::fs::remove_file(temporary);
        }
    }
}

fn random_persistence_nonce() -> u128 {
    rand::rng().random()
}

#[derive(Debug)]
struct PreparedCreateExhausted {
    target: PathBuf,
    attempts: u64,
    last_candidate: PathBuf,
    source: std::io::Error,
}

impl std::fmt::Display for PreparedCreateExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to prepare {} after {} create_new attempts; last candidate {}",
            self.target.display(),
            self.attempts,
            self.last_candidate.display()
        )
    }
}

impl std::error::Error for PreparedCreateExhausted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn create_prepared(
    target: &Path,
    pid: u32,
    nonce: u128,
    next_sequence: &mut impl FnMut() -> u64,
) -> Result<(std::fs::File, PathBuf), std::io::Error> {
    let mut last_collision = None;
    for _ in 0..PREPARED_CREATE_ATTEMPTS {
        let temporary = persistence_temporary(target, pid, nonce, next_sequence());
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some((temporary, error));
            }
            Err(error) => return Err(error),
        }
    }
    let Some((last_candidate, source)) = last_collision else {
        return Err(std::io::Error::other(
            "prepared persistence retry budget must be nonzero",
        ));
    };
    Err(std::io::Error::new(
        source.kind(),
        PreparedCreateExhausted {
            target: target.to_path_buf(),
            attempts: PREPARED_CREATE_ATTEMPTS,
            last_candidate,
            source,
        },
    ))
}

fn persistence_temporary(target: &Path, pid: u32, nonce: u128, sequence: u64) -> PathBuf {
    let mut name = target
        .file_name()
        .map_or_else(|| "profile".into(), std::ffi::OsStr::to_os_string);
    name.push(format!(".prepared-{pid}-{nonce:032x}-{sequence}"));
    target.with_file_name(name)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the cross-platform contract reports Unix directory sync failures; unsupported platforms are a no-op"
)]
fn sync_parent(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        std::fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Synced profile files and shadow captures awaiting terminal commit.
pub(super) struct PreparedPersistence {
    files: Vec<PreparedFile>,
    captures: Vec<crate::config_apply::ShadowCapture>,
}

/// Whether failed persistence left every authoritative file unchanged.
pub(super) enum PersistenceCommitError {
    /// Every authoritative file still has its original contents.
    Determinate(GatewayError),
    /// At least one authoritative file may contain committed contents.
    Indeterminate(GatewayError),
}

impl PreparedPersistence {
    async fn prepare(
        state: &AppState,
        name: &ProfileName,
        persistence: StatePersistence,
    ) -> Result<Self, GatewayError> {
        let mut plans = Vec::new();
        let mut captures = Vec::new();
        match persistence {
            StatePersistence::None => {}
            StatePersistence::Write => {
                if let Some(config) = state.config.as_ref() {
                    let contents = gateway_config::ProfileState::new(name)
                        .to_toml_string()
                        .map_err(crate::config_write::config_write_error)?;
                    plans.push((gateway_config::profile_state_path(&config.path), contents));
                }
            }
            StatePersistence::Promote(selected) => {
                plans.extend(
                    selected
                        .iter()
                        .map(|capture| (capture.real_path.clone(), capture.contents.clone())),
                );
                captures = selected;
            }
        }
        let files = tokio::task::spawn_blocking(move || {
            plans
                .into_iter()
                .map(|(target, contents)| PreparedFile::prepare(target, contents))
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
        Ok(Self { files, captures })
    }

    /// Atomically replaces each target and retires matching shadows.
    pub(super) async fn commit(self) -> Result<(), PersistenceCommitError> {
        tokio::task::spawn_blocking(move || self.commit_blocking())
            .await
            .map_err(|join| {
                PersistenceCommitError::Indeterminate(GatewayError::ConfigWriteIo(Box::new(join)))
            })?
    }

    fn commit_blocking(mut self) -> Result<(), PersistenceCommitError> {
        for file in &mut self.files {
            if let Err(error) = file.commit() {
                let error = GatewayError::ConfigWriteIo(Box::new(error));
                return if self.files.iter().all(PreparedFile::still_original) {
                    Err(PersistenceCommitError::Determinate(error))
                } else {
                    Err(PersistenceCommitError::Indeterminate(error))
                };
            }
        }
        for file in &self.files {
            if !file.has_committed_contents() {
                return Err(PersistenceCommitError::Indeterminate(
                    GatewayError::ConfigWriteIo(Box::new(std::io::Error::other(
                        "profile persistence could not verify committed contents",
                    ))),
                ));
            }
            sync_parent(file.target()).map_err(|error| {
                PersistenceCommitError::Indeterminate(GatewayError::ConfigWriteIo(Box::new(error)))
            })?;
        }
        for capture in &self.captures {
            let shadow = gateway_config::shadow_path(&capture.real_path);
            match std::fs::read_to_string(&shadow) {
                Ok(current) if current == capture.contents => {
                    if let Err(error) = std::fs::remove_file(&shadow)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(PersistenceCommitError::Indeterminate(
                            GatewayError::ConfigWriteIo(Box::new(error)),
                        ));
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(PersistenceCommitError::Indeterminate(
                        GatewayError::ConfigWriteIo(Box::new(error)),
                    ));
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    /// Prepares one persistence target for a terminal-commit test.
    pub(super) fn for_test(target: PathBuf, contents: String) -> Result<Self, GatewayError> {
        Ok(Self {
            files: vec![PreparedFile::prepare(target, contents)?],
            captures: Vec::new(),
        })
    }

    #[cfg(test)]
    /// Removes every owned temporary to force a commit failure.
    pub(super) fn discard_temporaries(&self) {
        for file in &self.files {
            file.discard_temporary();
        }
    }
}

/// Everything target preparation resolves for the later phases.
pub(super) struct SwitchTarget {
    /// The selected target configuration.
    pub(super) config: Config,
    /// Remote routing published at cutover and extended at terminal commit.
    pub(super) remote_routing: Routing,
    #[cfg(feature = "web-search")]
    /// Web-search state published at terminal commit.
    pub(super) web_search: Option<Arc<WebSearchState>>,
    /// Model names admitted by the selected profile.
    pub(super) allowlist: Option<Vec<String>>,
    loading: BTreeSet<String>,
    #[cfg(feature = "stt")]
    speech: Option<gateway_stt::PreparedSpeech>,
}

#[derive(Debug, Clone, Copy)]
struct StopSet {
    #[cfg(feature = "local")]
    local: bool,
    #[cfg(feature = "stt")]
    stt: bool,
}

impl StopSet {
    fn is_empty(self) -> bool {
        let any = false;
        #[cfg(feature = "local")]
        let any = any || self.local;
        #[cfg(feature = "stt")]
        let any = any || self.stt;
        !any
    }
}

/// Live runtime state captured immediately before cutover.
pub(super) struct PriorRuntimeSnapshot {
    routing: Arc<Routing>,
    routing_was_empty: bool,
    config: Arc<Config>,
    #[cfg(feature = "web-search")]
    web_search: Option<Arc<WebSearchState>>,
    profile_name: Option<String>,
    model_allowlist: Option<Vec<String>>,
    loading: BTreeSet<String>,
    #[cfg(feature = "local")]
    restart_local: bool,
}

/// A transaction whose target and persistence are ready but whose live-state
/// cutover has not happened.
pub(super) struct PreparedPhase {
    state: AppState,
    name: ProfileName,
    tree: ProgressTree,
    target: SwitchTarget,
    stop: StopSet,
    persistence: PreparedPersistence,
    token: CancellationToken,
    download_after_cutover: bool,
}

/// A transaction whose interim live-state cutover has happened.
pub(super) struct CutoverPhase {
    state: AppState,
    name: ProfileName,
    tree: ProgressTree,
    target: SwitchTarget,
    persistence: PreparedPersistence,
    prior: PriorRuntimeSnapshot,
    token: CancellationToken,
}

impl PreparedPhase {
    /// Consumes the prepared phase and produces the only value that can enter
    /// runtime staging.
    pub(super) async fn cut_over(self) -> Result<CutoverPhase, GatewayError> {
        let prior = capture_runtime_snapshot(&self.state).await;
        if let Err(error) = cut_over(
            &self.state,
            &self.target,
            &self.tree,
            self.stop,
            &self.token,
        )
        .await
        {
            return Err(restore_or_shutdown(&self.state, &self.token, prior, error).await);
        }
        let cutover = CutoverPhase {
            state: self.state,
            name: self.name,
            tree: self.tree,
            target: self.target,
            persistence: self.persistence,
            prior,
            token: self.token,
        };
        if self.download_after_cutover {
            #[cfg(test)]
            cutover
                .state
                .park_at(crate::switch_park::SwitchPhase::Download)
                .await;
            match download_artifacts(&cutover.target, &cutover.tree, &cutover.token).await {
                Ok(()) => {}
                Err(error) => return Err(cutover.restore_or_shutdown(error).await),
            }
        }
        Ok(cutover)
    }
}

impl CutoverPhase {
    /// Reports cancellation through the transaction-owned token.
    pub(super) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Builds the profile-specific cancellation result.
    pub(super) fn cancellation_error(&self) -> GatewayError {
        switch_cancelled(&self.name)
    }

    /// Borrows the transaction-owned cancellation token.
    pub(super) fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Borrows the selected target configuration for runtime staging.
    pub(super) fn config(&self) -> &Config {
        &self.target.config
    }

    /// Borrows the operation tree for runtime staging.
    pub(super) fn tree(&self) -> &ProgressTree {
        &self.tree
    }

    #[cfg(feature = "stt")]
    /// Transfers prepared speech into runtime staging exactly once.
    pub(super) fn take_prepared_speech(
        &mut self,
    ) -> Result<gateway_stt::PreparedSpeech, GatewayError> {
        self.target.speech.take().ok_or_else(|| {
            GatewayError::switch_failed(
                "stage-stt",
                std::io::Error::other("speech preparation was already consumed"),
            )
        })
    }

    /// Restores the prior runtime or requests controlled shutdown.
    pub(super) async fn restore_or_shutdown(self, failure: GatewayError) -> GatewayError {
        restore_or_shutdown(&self.state, &self.token, self.prior, failure).await
    }

    /// Hands resources to the unchanged terminal staging and commit path.
    pub(super) fn into_terminal_parts(
        self,
    ) -> (
        SwitchTarget,
        PreparedPersistence,
        PriorRuntimeSnapshot,
        CancellationToken,
    ) {
        (self.target, self.persistence, self.prior, self.token)
    }
}

/// Resolves and persists the target into a value that alone can cut over.
pub(super) async fn prepare(
    state: &AppState,
    name: ProfileName,
    tree: ProgressTree,
    candidate: Option<Config>,
    persistence: impl FnOnce() -> StatePersistence,
    token: &CancellationToken,
) -> Result<PreparedPhase, GatewayError> {
    let token = token.clone();
    if token.is_cancelled() {
        return Err(switch_cancelled(&name));
    }
    let target = prepare_target(state, &name, &tree, candidate).await?;
    if token.is_cancelled() {
        return Err(switch_cancelled(&name));
    }
    let stop = stop_set(state).await;
    let download_after_cutover = stop.is_empty();
    let persistence = persistence();
    if !download_after_cutover {
        #[cfg(test)]
        state
            .park_at(crate::switch_park::SwitchPhase::Download)
            .await;
        download_artifacts(&target, &tree, &token).await?;
        if token.is_cancelled() {
            return Err(switch_cancelled(&name));
        }
    }
    let persistence = PreparedPersistence::prepare(state, &name, persistence).await?;
    if token.is_cancelled() {
        return Err(switch_cancelled(&name));
    }
    Ok(PreparedPhase {
        state: state.clone(),
        name,
        tree,
        target,
        stop,
        persistence,
        token,
        download_after_cutover,
    })
}

async fn prepare_target(
    state: &AppState,
    name: &ProfileName,
    tree: &ProgressTree,
    candidate: Option<Config>,
) -> Result<SwitchTarget, GatewayError> {
    let loading = tree.register("loading-profile", 1.0);
    let catalog = match candidate {
        Some(config) => config,
        None => state.live.read().await.config.as_ref().clone(),
    };
    let (config, remote_routing) = select_target(&catalog, name, &loading)?;
    #[cfg(not(feature = "local"))]
    if !config.local_models().is_empty() {
        loading.fail();
        return Err(GatewayError::switch_failed(
            "start-local",
            std::io::Error::other(LOCAL_MODELS_UNSUPPORTED),
        ));
    }
    #[cfg(feature = "stt")]
    let speech = {
        let service = state.speech.clone();
        let config = config.clone();
        let progress = loading.clone();
        tokio::task::spawn_blocking(move || service.prepare(&config, Some(&progress)))
            .await
            .map_err(|error| GatewayError::switch_failed("prepare-stt-task", error))?
            .map_err(|error| GatewayError::switch_failed("prepare-stt", error))?
    };
    loading.complete();

    #[cfg(feature = "web-search")]
    let web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    let allowlist = config
        .active_profile()
        .map(|profile| profile.models().to_vec());
    let loading = config
        .local_models()
        .iter()
        .map(|model| model.name().to_owned())
        .collect();
    Ok(SwitchTarget {
        config,
        remote_routing,
        #[cfg(feature = "web-search")]
        web_search,
        allowlist,
        loading,
        #[cfg(feature = "stt")]
        speech: Some(speech),
    })
}

fn select_target(
    catalog: &Config,
    name: &ProfileName,
    loading: &ProgressHandle,
) -> Result<(Config, Routing), GatewayError> {
    if !catalog
        .profiles()
        .iter()
        .any(|profile| profile.name() == name.as_str())
    {
        loading.fail();
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }
    let config = match catalog.select_profile(name) {
        Ok(config) => config,
        Err(error) => {
            loading.fail();
            return Err(GatewayError::switch_failed("select-profile", error));
        }
    };
    #[cfg(not(feature = "stt"))]
    if !config.stt_models().is_empty() {
        loading.fail();
        return Err(GatewayError::switch_failed(
            "start-stt",
            std::io::Error::other(STT_RUNTIME_UNAVAILABLE),
        ));
    }
    let remote_routing = match Routing::from_config(&config) {
        Ok(routing) => routing,
        Err(error) => {
            loading.fail();
            return Err(GatewayError::switch_failed("build-routing", error));
        }
    };
    Ok((config, remote_routing))
}

fn switch_cancelled(name: &ProfileName) -> GatewayError {
    GatewayError::CommandCancelled(format!("load-profile: {name}"))
}

#[cfg(test)]
/// Resolves only a target for tests of the unchanged terminal commit.
pub(super) async fn prepare_target_for_test(
    state: &AppState,
    name: &ProfileName,
    tree: &ProgressTree,
    candidate: Option<Config>,
) -> Result<SwitchTarget, GatewayError> {
    prepare_target(state, name, tree, candidate).await
}

#[cfg(any(feature = "local", feature = "stt"))]
async fn stop_set(state: &AppState) -> StopSet {
    #[cfg(feature = "local")]
    let live = state.live.read().await;
    StopSet {
        #[cfg(feature = "local")]
        local: live.local.child_count() > 0,
        #[cfg(feature = "stt")]
        stt: state.speech.status().ready(),
    }
}

#[cfg(not(any(feature = "local", feature = "stt")))]
async fn stop_set(_state: &AppState) -> StopSet {
    StopSet {}
}

#[cfg(feature = "local")]
async fn download_artifacts(
    target: &SwitchTarget,
    tree: &ProgressTree,
    token: &CancellationToken,
) -> Result<(), GatewayError> {
    if target.config.local_models().is_empty() {
        return Ok(());
    }
    let downloading = tree.register("downloading-models", 5.0);
    let config = target.config.clone();
    let progress = downloading.clone();
    let worker_token = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::local::LocalRuntime::provision_artifacts_with_cancellation(
            &config,
            Some(&progress),
            &worker_token,
        )
    })
    .await;
    match result {
        Ok(Ok(failures)) if failures.is_empty() => {
            downloading.complete();
            Ok(())
        }
        Ok(Ok(failures)) => {
            for failure in &failures {
                tracing::warn!(
                    model = failure.model(),
                    error = %failure.error(),
                    "local model artifact did not provision; the start reports it"
                );
            }
            downloading.fail();
            Ok(())
        }
        Ok(Err(error)) => {
            downloading.fail();
            Err(GatewayError::switch_failed("download-models", error))
        }
        Err(error) => {
            downloading.fail();
            Err(GatewayError::switch_failed("download-models-task", error))
        }
    }
}

#[cfg(not(feature = "local"))]
async fn download_artifacts(
    _target: &SwitchTarget,
    _tree: &ProgressTree,
    _token: &CancellationToken,
) -> Result<(), GatewayError> {
    Ok(())
}

async fn cut_over(
    state: &AppState,
    target: &SwitchTarget,
    tree: &ProgressTree,
    stop: StopSet,
    token: &CancellationToken,
) -> Result<(), GatewayError> {
    let _switch = state.switch.lock().await;
    #[cfg(test)]
    {
        state
            .park_at(crate::switch_park::SwitchPhase::CutOver)
            .await;
    }
    tokio::select! {
        () = drain_inference(state) => {}
        () = token.cancelled() => {
            return Err(GatewayError::CommandCancelled("profile switch".to_owned()));
        }
    }
    let stopping = if stop.is_empty() {
        None
    } else {
        Some(tree.register("stopping-models", 2.0))
    };
    let old = {
        let mut live = state.live.write().await;
        live.routing = Arc::new(target.remote_routing.clone());
        live.loading.clone_from(&target.loading);
        if stopping.is_none() {
            None
        } else {
            Some(OldRuntimes {
                #[cfg(feature = "local")]
                local: std::mem::replace(&mut live.local, LocalRuntime::empty()),
            })
        }
    };
    let (Some(stopping), Some(old)) = (stopping, old) else {
        return Ok(());
    };
    match tokio::task::spawn_blocking(move || old.shutdown()).await {
        Ok(Ok(())) => {
            stopping.complete();
            Ok(())
        }
        Ok(Err(error)) => {
            stopping.fail();
            Err(GatewayError::switch_failed("shutdown-local", error))
        }
        Err(error) => {
            stopping.fail();
            Err(GatewayError::switch_failed("shutdown-local-task", error))
        }
    }
}

struct OldRuntimes {
    #[cfg(feature = "local")]
    local: LocalRuntime,
}

impl OldRuntimes {
    fn shutdown(self) -> Result<(), shared_protocol::ShutdownError> {
        #[cfg(feature = "local")]
        let result = self.local.shutdown();
        #[cfg(not(feature = "local"))]
        let result = Ok(());
        result
    }
}

async fn drain_inference(state: &AppState) {
    if !state
        .in_flight
        .drain_or_cancel(std::time::Duration::from_secs(30))
        .await
    {
        tracing::warn!(
            "profile-switch cancellation grace expired; stopping local children with request guards still registered"
        );
    }
}

async fn capture_runtime_snapshot(state: &AppState) -> PriorRuntimeSnapshot {
    let live = state.live.read().await;
    PriorRuntimeSnapshot {
        routing_was_empty: live.routing.models().is_empty(),
        routing: Arc::clone(&live.routing),
        config: Arc::clone(&live.config),
        #[cfg(feature = "web-search")]
        web_search: live.web_search.clone(),
        profile_name: live.profile_name.clone(),
        model_allowlist: live.model_allowlist.clone(),
        loading: live.loading.clone(),
        #[cfg(feature = "local")]
        restart_local: !live.local.models().is_empty(),
    }
}

async fn restore_runtime_snapshot(
    state: &AppState,
    prior: PriorRuntimeSnapshot,
) -> Result<(), GatewayError> {
    #[cfg(feature = "local")]
    let local = if prior.restart_local {
        let config = Arc::clone(&prior.config);
        tokio::time::timeout(
            STAGE_TIMEOUT,
            tokio::task::spawn_blocking(move || LocalRuntime::start(&config, None)),
        )
        .await
        .map_err(|_| {
            GatewayError::switch_failed(
                "rollback-local-timeout",
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "old local runtime reconstruction exceeded its startup deadline",
                ),
            )
        })?
        .map_err(|join| GatewayError::switch_failed("rollback-local-task", join))?
        .map_err(|error| GatewayError::switch_failed("rollback-local", error))?
    } else {
        LocalRuntime::empty()
    };
    let mut live = state.live.write().await;
    if !prior.routing_was_empty {
        live.routing = prior.routing;
    }
    live.config = prior.config;
    #[cfg(feature = "web-search")]
    {
        live.web_search = prior.web_search;
    }
    live.profile_name = prior.profile_name;
    live.model_allowlist = prior.model_allowlist;
    live.loading = prior.loading;
    #[cfg(feature = "local")]
    {
        live.local = local;
    }
    Ok(())
}

/// Restores a prior snapshot, escalating failed restoration to shutdown.
pub(super) async fn restore_or_shutdown(
    state: &AppState,
    token: &CancellationToken,
    prior: PriorRuntimeSnapshot,
    failure: GatewayError,
) -> GatewayError {
    match restore_runtime_snapshot(state, prior).await {
        Ok(()) => failure,
        Err(rollback) => {
            token.cancel();
            state.shutdown.fire();
            #[cfg(feature = "stt")]
            state.speech.shutdown();
            GatewayError::switch_failed("rollback-profile", rollback)
        }
    }
}

#[cfg(test)]
mod tests {
    use gateway_config::{Config, ProfileName};
    use tokio_util::sync::CancellationToken;

    use crate::error::GatewayError;
    use crate::test_support::app_state;

    fn state() -> crate::AppState {
        let catalog = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"alpha-model\"\ndescription = \"alpha\"\ncontext = 1024\nupstream = \"alpha\"\nendpoints = [\"fake\"]\n\
             [[model]]\nname = \"beta-model\"\ndescription = \"beta\"\ncontext = 1024\nupstream = \"beta\"\nendpoints = [\"fake\"]\n\
             [[profile]]\nname = \"alpha\"\nmodels = [\"alpha-model\"]\n\
             [[profile]]\nname = \"beta\"\nmodels = [\"beta-model\"]\n",
        )
        .expect("catalog parses");
        let config = catalog
            .select_profile(&ProfileName::parse("alpha").expect("profile name"))
            .expect("alpha profile selects");
        app_state(config, None)
    }

    #[test]
    fn prepared_file_retries_deterministic_collisions_without_claiming_residue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        std::fs::write(&target, "old").expect("write target");
        let pid = 41;
        let nonce = 0x1234;
        let collision = super::persistence_temporary(&target, pid, nonce, 7);
        let owned = super::persistence_temporary(&target, pid, nonce, 8);
        std::fs::write(&collision, "crash residue").expect("write collision");
        let mut sequences = [7, 8].into_iter();

        let prepared =
            super::PreparedFile::prepare_with_names(target, "new".to_owned(), pid, nonce, || {
                sequences.next().expect("bounded sequence")
            })
            .expect("collision retries");

        assert_eq!(
            std::fs::read_to_string(&collision).expect("read residue"),
            "crash residue"
        );
        assert_eq!(
            std::fs::read_to_string(&owned).expect("read preparation"),
            "new"
        );
        drop(prepared);
        assert!(collision.exists(), "unowned residue remains");
        assert!(!owned.exists(), "owned preparation is cleaned");
    }

    #[test]
    fn process_name_source_is_stable_full_width_and_unique_across_pid_reuse() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 73;
        let first_nonce = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;
        let second_nonce = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdef;
        let first_nonce_calls = std::cell::Cell::new(0);
        let first_source = super::ProcessPreparationNames::new(pid, || {
            first_nonce_calls.set(first_nonce_calls.get() + 1);
            first_nonce
        });
        let second_source = super::ProcessPreparationNames::new(pid, || second_nonce);

        let first = super::PreparedFile::prepare_with_name_source(
            target.clone(),
            "first preparation".to_owned(),
            &first_source,
        )
        .expect("first process prepares");
        let next = super::PreparedFile::prepare_with_name_source(
            target.clone(),
            "next preparation".to_owned(),
            &first_source,
        )
        .expect("same process prepares again");
        let reused = super::PreparedFile::prepare_with_name_source(
            target,
            "reused PID preparation".to_owned(),
            &second_source,
        )
        .expect("reused PID prepares");

        assert_eq!(first_nonce_calls.get(), 1, "one nonce per process source");
        assert_eq!(
            first
                .temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-0123456789abcdeffedcba9876543210-0"
            ))
        );
        assert_eq!(
            next.temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-0123456789abcdeffedcba9876543210-1"
            ))
        );
        assert_eq!(
            reused
                .temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-fedcba98765432100123456789abcdef-0"
            ))
        );
    }

    #[test]
    fn process_nonce_separates_pid_reuse_from_crash_residue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 73;
        let crashed = super::persistence_temporary(&target, pid, 0xaaaa, 0);
        let current = super::persistence_temporary(&target, pid, 0xbbbb, 0);
        std::fs::write(&crashed, "prior process").expect("write crash residue");

        let prepared = super::PreparedFile::prepare_with_names(
            target,
            "current process".to_owned(),
            pid,
            0xbbbb,
            || 0,
        )
        .expect("reused PID prepares");

        assert_eq!(
            std::fs::read_to_string(&crashed).expect("read crash residue"),
            "prior process"
        );
        assert_eq!(
            std::fs::read_to_string(&current).expect("read current preparation"),
            "current process"
        );
        drop(prepared);
        assert!(crashed.exists(), "prior process residue remains");
    }

    #[test]
    fn prepared_file_bounds_collision_retries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 97;
        let nonce = 0xcafe;
        for sequence in 0..super::PREPARED_CREATE_ATTEMPTS {
            std::fs::write(
                super::persistence_temporary(&target, pid, nonce, sequence),
                format!("residue {sequence}"),
            )
            .expect("write residue");
        }
        let mut sequence = 0_u64;

        let error = super::PreparedFile::prepare_with_names(
            target.clone(),
            "new".to_owned(),
            pid,
            nonce,
            || {
                let current = sequence;
                sequence += 1;
                current
            },
        )
        .expect_err("retry budget exhausts");

        let GatewayError::ConfigWriteIo(error) = error else {
            panic!("collision exhaustion returns an I/O error");
        };
        let error = error.downcast_ref::<std::io::Error>().expect("I/O source");
        let last_candidate =
            super::persistence_temporary(&target, pid, nonce, super::PREPARED_CREATE_ATTEMPTS - 1);
        assert_eq!(
            error.to_string(),
            format!(
                "failed to prepare {} after {} create_new attempts; last candidate {}",
                target.display(),
                super::PREPARED_CREATE_ATTEMPTS,
                last_candidate.display()
            )
        );
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let context = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<super::PreparedCreateExhausted>())
            .expect("collision exhaustion context");
        assert_eq!(context.attempts, super::PREPARED_CREATE_ATTEMPTS);
        assert_eq!(context.last_candidate, last_candidate);
        let collision = std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .expect("final collision source");
        assert_eq!(collision.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(sequence, super::PREPARED_CREATE_ATTEMPTS);
        for residue in 0..super::PREPARED_CREATE_ATTEMPTS {
            assert_eq!(
                std::fs::read_to_string(
                    super::persistence_temporary(&target, pid, nonce, residue,)
                )
                .expect("read residue"),
                format!("residue {residue}")
            );
        }
    }

    #[test]
    fn successful_commit_releases_temporary_path_ownership() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        std::fs::write(&target, "old").expect("write target");
        let temporary = super::persistence_temporary(&target, 101, 0xfeed, 3);
        let mut prepared = super::PreparedFile::prepare_with_names(
            target.clone(),
            "new".to_owned(),
            101,
            0xfeed,
            || 3,
        )
        .expect("prepare");

        prepared.commit().expect("commit");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "new"
        );
        assert!(!temporary.exists(), "rename consumes preparation");
        std::fs::write(&temporary, "later owner").expect("replace temporary path");
        drop(prepared);
        assert_eq!(
            std::fs::read_to_string(&temporary).expect("read later owner"),
            "later owner"
        );
    }

    #[test]
    fn persistence_failure_classification_distinguishes_untouched_from_uncertain_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        std::fs::write(&target, "active_profile = \"alpha\"\n").expect("write old state");

        let determinate = super::PreparedPersistence::for_test(
            target.clone(),
            "active_profile = \"beta\"\n".to_owned(),
        )
        .expect("prepare determinate fixture");
        determinate.discard_temporaries();
        let error = determinate
            .commit_blocking()
            .expect_err("missing temporary prevents commit");
        assert!(matches!(
            error,
            super::PersistenceCommitError::Determinate(_)
        ));

        let indeterminate = super::PreparedPersistence::for_test(
            target.clone(),
            "active_profile = \"beta\"\n".to_owned(),
        )
        .expect("prepare indeterminate fixture");
        std::fs::write(&target, "unrecognized contents").expect("replace authoritative state");
        indeterminate.discard_temporaries();
        let error = indeterminate
            .commit_blocking()
            .expect_err("missing temporary prevents commit");
        assert!(matches!(
            error,
            super::PersistenceCommitError::Indeterminate(_)
        ));
    }

    #[tokio::test]
    async fn preparation_produces_a_prepared_phase_without_publishing_target() {
        let state = state();
        let tree = state.hub.operation();
        let token = CancellationToken::new();
        let prepared = super::prepare(
            &state,
            ProfileName::parse("beta").expect("profile name"),
            tree,
            None,
            || super::StatePersistence::None,
            &token,
        )
        .await
        .expect("preparation succeeds");

        assert_eq!(
            prepared
                .target
                .config
                .active_profile()
                .expect("target profile")
                .name(),
            "beta"
        );
        let live = state.live.read().await;
        assert!(live.routing.model("alpha-model").is_ok());
        assert!(live.routing.model("beta-model").is_err());
    }

    #[tokio::test]
    async fn prepared_phase_transitions_once_to_cutover_with_prior_snapshot() {
        let state = state();
        let tree = state.hub.operation();
        let token = CancellationToken::new();
        let prepared = super::prepare(
            &state,
            ProfileName::parse("beta").expect("profile name"),
            tree,
            None,
            || super::StatePersistence::None,
            &token,
        )
        .await
        .expect("preparation succeeds");

        let cutover = prepared.cut_over().await.expect("cutover succeeds");

        assert!(cutover.prior.routing.model("alpha-model").is_ok());
        assert!(cutover.prior.routing.model("beta-model").is_err());
        let live = state.live.read().await;
        assert!(live.routing.model("alpha-model").is_err());
        assert!(live.routing.model("beta-model").is_ok());
    }
}
