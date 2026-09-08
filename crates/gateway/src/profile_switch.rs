//! Private profile-switch transaction.
//!
//! Prepared, cutover, staged, committed, rolled-back, indeterminate, and
//! terminal values own each phase's resources and legal transitions.

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
#[cfg(feature = "stt")]
use gateway_stt::{SpeechReplacement, SpeechService};
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
    speech: gateway_stt::PreparedSpeech,
}

/// Target data that remains after the prepared speech artifact enters staging.
pub(super) struct StagedTarget {
    config: Config,
    remote_routing: Routing,
    #[cfg(feature = "web-search")]
    web_search: Option<Arc<WebSearchState>>,
    allowlist: Option<Vec<String>>,
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
    #[cfg(any(test, not(feature = "local")))]
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

/// Cutover ownership after the prepared speech artifact has entered staging.
struct CutoverOwner {
    state: AppState,
    name: ProfileName,
    target: StagedTarget,
    persistence: PreparedPersistence,
    prior: PriorRuntimeSnapshot,
    token: CancellationToken,
}

/// A transaction whose target runtimes are staged but not persisted or
/// published.
struct StagedPhase {
    state: AppState,
    name: ProfileName,
    target: StagedTarget,
    replacement: RuntimeReplacement,
    persistence: PreparedPersistence,
    prior: PriorRuntimeSnapshot,
    token: CancellationToken,
}

/// Staged ownership after persistence has been consumed.
struct CommitTail {
    state: AppState,
    name: ProfileName,
    target: StagedTarget,
    replacement: RuntimeReplacement,
    prior: PriorRuntimeSnapshot,
    token: CancellationToken,
}

/// Persisted ownership awaiting atomic runtime and live-state publication.
struct PublicationPhase {
    state: AppState,
    name: ProfileName,
    target: StagedTarget,
    replacement: RuntimeReplacement,
    #[cfg(feature = "stt")]
    token: CancellationToken,
    routing: Routing,
}

/// A transaction that atomically persisted and published its target.
#[derive(Debug)]
struct CommittedPhase {
    report: StartReport,
}

/// A transaction that reconstructed and republished its prior runtime.
#[derive(Debug)]
struct RolledBackPhase {
    error: GatewayError,
}

/// A transaction whose runtime or persistence could not be proven and which
/// requested controlled shutdown.
#[derive(Debug)]
struct IndeterminatePhase {
    error: GatewayError,
}

/// The sole terminal owner returned by every post-preparation branch.
#[derive(Debug)]
enum TerminalPhase {
    Committed(CommittedPhase),
    RolledBack(RolledBackPhase),
    Indeterminate(IndeterminatePhase),
}

impl TerminalPhase {
    fn finish(self) -> Result<StartReport, GatewayError> {
        match self {
            Self::Committed(phase) => Ok(phase.report),
            Self::RolledBack(phase) => Err(phase.error),
            Self::Indeterminate(phase) => Err(phase.error),
        }
    }
}

/// What committed staging reported for local model startup.
#[derive(Debug)]
pub(super) struct StartReport {
    #[cfg(feature = "local")]
    loaded: Vec<String>,
    #[cfg(feature = "local")]
    failed: Vec<String>,
}

/// The runtimes phase 4 started, swapped into live state only at commit.
pub(super) struct RuntimeReplacement {
    #[cfg(feature = "local")]
    pub(super) local: LocalRuntime,
    #[cfg(feature = "local")]
    pub(super) start_failures: Vec<crate::local::LocalStartFailure>,
    #[cfg(feature = "stt")]
    pub(super) speech: SpeechReplacement,
}

#[derive(Debug)]
#[cfg_attr(
    not(any(feature = "local", feature = "stt")),
    expect(
        dead_code,
        reason = "the featureless stage stub cannot produce either runtime failure classification"
    )
)]
pub(super) enum RuntimeStageFailure {
    Determinate(GatewayError),
    Indeterminate(GatewayError),
}

impl PreparedPhase {
    /// Consumes the prepared phase and produces the only value that can enter
    /// runtime staging.
    async fn cut_over(self) -> Result<CutoverPhase, TerminalPhase> {
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
            return Err(self.roll_back(prior, error).await);
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
                Err(error) => return Err(cutover.roll_back(error).await),
            }
        }
        Ok(cutover)
    }

    async fn roll_back(self, prior: PriorRuntimeSnapshot, failure: GatewayError) -> TerminalPhase {
        match restore_runtime_snapshot(&self.state, prior).await {
            Ok(()) => TerminalPhase::RolledBack(RolledBackPhase { error: failure }),
            Err(rollback) => self.into_indeterminate("rollback-profile", rollback),
        }
    }

    fn into_indeterminate(self, phase: &'static str, failure: GatewayError) -> TerminalPhase {
        TerminalPhase::Indeterminate(IndeterminatePhase {
            error: request_fatal_shutdown(&self.state, &self.token, phase, failure),
        })
    }
}

impl CutoverPhase {
    async fn stage(self) -> Result<StagedPhase, TerminalPhase> {
        if self.token.is_cancelled() {
            let error = switch_cancelled(&self.name);
            return Err(self.roll_back(error).await);
        }
        #[cfg(test)]
        {
            self.state
                .park_at(crate::switch_park::SwitchPhase::Spawn)
                .await;
        }
        let Some(deadline) = std::time::Instant::now().checked_add(STAGE_TIMEOUT) else {
            let error = GatewayError::switch_failed(
                "stage-profile-deadline",
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "profile staging deadline could not be represented",
                ),
            );
            return Err(self.roll_back(error).await);
        };
        let CutoverPhase {
            state,
            name,
            tree,
            target,
            persistence,
            prior,
            token,
        } = self;
        let SwitchTarget {
            config,
            remote_routing,
            #[cfg(feature = "web-search")]
            web_search,
            allowlist,
            loading: _,
            #[cfg(feature = "stt")]
                speech: prepared_speech,
        } = target;
        let owner = CutoverOwner {
            state,
            name,
            target: StagedTarget {
                config,
                remote_routing,
                #[cfg(feature = "web-search")]
                web_search,
                allowlist,
            },
            persistence,
            prior,
            token,
        };
        #[cfg(test)]
        if owner
            .state
            .has_switch_fault(crate::switch_park::SwitchFault::StageIndeterminate)
        {
            return Err(owner.into_indeterminate(
                "stage-profile-timeout",
                GatewayError::switch_failed(
                    "start-runtime-timeout",
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "injected non-preemptible runtime startup timeout",
                    ),
                ),
            ));
        }
        let replacement = match spawn_runtimes(
            &owner.target.config,
            #[cfg(feature = "stt")]
            owner.state.speech.clone(),
            #[cfg(feature = "stt")]
            prepared_speech,
            &tree,
            &owner.token,
            deadline,
        )
        .await
        {
            Ok(replacement) => replacement,
            Err(RuntimeStageFailure::Determinate(error)) => {
                return Err(owner.roll_back(error).await);
            }
            Err(RuntimeStageFailure::Indeterminate(error)) => {
                return Err(owner.into_indeterminate("stage-profile-timeout", error));
            }
        };
        let staged = owner.into_staged(replacement);
        if staged.token.is_cancelled() {
            return Err(staged.roll_back_after_stage(switch_cancelled).await);
        }
        Ok(staged)
    }

    async fn roll_back(self, failure: GatewayError) -> TerminalPhase {
        match restore_runtime_snapshot(&self.state, self.prior).await {
            Ok(()) => TerminalPhase::RolledBack(RolledBackPhase { error: failure }),
            Err(rollback) => TerminalPhase::Indeterminate(IndeterminatePhase {
                error: request_fatal_shutdown(
                    &self.state,
                    &self.token,
                    "rollback-profile",
                    rollback,
                ),
            }),
        }
    }
}

impl CutoverOwner {
    fn into_staged(self, replacement: RuntimeReplacement) -> StagedPhase {
        StagedPhase {
            state: self.state,
            name: self.name,
            target: self.target,
            replacement,
            persistence: self.persistence,
            prior: self.prior,
            token: self.token,
        }
    }

    async fn roll_back(self, failure: GatewayError) -> TerminalPhase {
        match restore_runtime_snapshot(&self.state, self.prior).await {
            Ok(()) => TerminalPhase::RolledBack(RolledBackPhase { error: failure }),
            Err(rollback) => TerminalPhase::Indeterminate(IndeterminatePhase {
                error: request_fatal_shutdown(
                    &self.state,
                    &self.token,
                    "rollback-profile",
                    rollback,
                ),
            }),
        }
    }

    fn into_indeterminate(self, phase: &'static str, failure: GatewayError) -> TerminalPhase {
        TerminalPhase::Indeterminate(IndeterminatePhase {
            error: request_fatal_shutdown(&self.state, &self.token, phase, failure),
        })
    }
}

impl StagedPhase {
    async fn commit(self) -> TerminalPhase {
        let state = self.state.clone();
        let _switch = state.switch.lock().await;
        #[cfg(test)]
        {
            state.park_at(crate::switch_park::SwitchPhase::Commit).await;
        }
        #[cfg(feature = "local")]
        let routing = match self
            .target
            .remote_routing
            .clone()
            .merge(self.replacement.local.models().iter().cloned())
        {
            Ok(routing) => routing,
            Err(error) => {
                let failure = GatewayError::switch_failed("merge-routing", error);
                return self.into_rollback(failure).finish().await;
            }
        };
        #[cfg(not(feature = "local"))]
        let routing = self.target.remote_routing.clone();
        if self.token.is_cancelled() {
            return self
                .into_rollback(GatewayError::CommandCancelled("profile switch".to_owned()))
                .finish()
                .await;
        }
        let publication_state = state.clone();
        let _publication = tokio::select! {
            biased;
            () = self.token.cancelled() => {
                return self
                    .into_rollback(GatewayError::CommandCancelled(
                        "profile switch".to_owned(),
                    ))
                    .finish()
                    .await;
            }
            guard = publication_state.apply.lock() => guard,
        };
        if self.token.is_cancelled() {
            return self
                .into_rollback(GatewayError::CommandCancelled("profile switch".to_owned()))
                .finish()
                .await;
        }
        let StagedPhase {
            state,
            name,
            target,
            replacement,
            persistence,
            prior,
            token,
        } = self;
        let tail = CommitTail {
            state,
            name,
            target,
            replacement,
            prior,
            token,
        };
        match persistence.commit().await {
            Ok(()) => {}
            Err(PersistenceCommitError::Determinate(error)) => {
                return tail.into_rollback(error).finish().await;
            }
            Err(PersistenceCommitError::Indeterminate(error)) => {
                return tail.into_indeterminate("persist-profile-indeterminate", error);
            }
        }
        let publication = tail.into_publication(routing);
        #[cfg(test)]
        {
            publication
                .state
                .park_at(crate::switch_park::SwitchPhase::Publish)
                .await;
        }
        publication.publish().await
    }

    async fn roll_back_after_stage(
        self,
        cancellation: impl FnOnce(&ProfileName) -> GatewayError,
    ) -> TerminalPhase {
        let failure = cancellation(&self.name);
        self.into_rollback(failure).finish().await
    }

    fn into_rollback(self, failure: GatewayError) -> RollbackOwner {
        let runtime_rollback = rollback_runtime(&self.state, self.replacement);
        RollbackOwner {
            state: self.state,
            prior: self.prior,
            token: self.token,
            failure,
            runtime_rollback,
        }
    }
}

impl CommitTail {
    fn into_rollback(self, failure: GatewayError) -> RollbackOwner {
        let runtime_rollback = rollback_runtime(&self.state, self.replacement);
        RollbackOwner {
            state: self.state,
            prior: self.prior,
            token: self.token,
            failure,
            runtime_rollback,
        }
    }

    fn into_indeterminate(self, phase: &'static str, failure: GatewayError) -> TerminalPhase {
        TerminalPhase::Indeterminate(IndeterminatePhase {
            error: request_fatal_shutdown(&self.state, &self.token, phase, failure),
        })
    }

    fn into_publication(self, routing: Routing) -> PublicationPhase {
        PublicationPhase {
            state: self.state,
            name: self.name,
            target: self.target,
            replacement: self.replacement,
            #[cfg(feature = "stt")]
            token: self.token,
            routing,
        }
    }
}

impl PublicationPhase {
    async fn publish(self) -> TerminalPhase {
        let report = start_report(&self.replacement);
        let PublicationPhase {
            state,
            name,
            target,
            #[cfg(any(feature = "local", feature = "stt"))]
            replacement,
            #[cfg(not(any(feature = "local", feature = "stt")))]
                replacement: _,
            #[cfg(feature = "stt")]
            token,
            routing,
        } = self;
        #[cfg(feature = "stt")]
        if let Err(error) = state.speech.commit_replacement(replacement.speech) {
            return TerminalPhase::Indeterminate(IndeterminatePhase {
                error: request_fatal_shutdown(
                    &state,
                    &token,
                    "publish-stt",
                    GatewayError::switch_failed("publish-stt", error),
                ),
            });
        }

        let mut live = state.live.write().await;
        live.routing = Arc::new(routing);
        live.config = Arc::new(target.config);
        #[cfg(feature = "web-search")]
        {
            live.web_search = target.web_search;
        }
        #[cfg(feature = "local")]
        {
            live.local = replacement.local;
        }
        live.profile_name = Some(name.to_string());
        live.model_allowlist = target.allowlist;
        live.loading.clear();
        TerminalPhase::Committed(CommittedPhase { report })
    }
}

/// Runs the private transaction and returns only its externally visible
/// profile outcome.
pub(super) async fn run(
    state: &AppState,
    name: ProfileName,
    tree: ProgressTree,
    candidate: Option<Config>,
    persistence: impl FnOnce() -> StatePersistence,
    token: &CancellationToken,
) -> Result<String, GatewayError> {
    let prepared = prepare(state, name.clone(), tree, candidate, persistence, token).await?;
    let cutover = match prepared.cut_over().await {
        Ok(phase) => phase,
        Err(terminal) => return settle_terminal(terminal, &name),
    };
    let staged = match cutover.stage().await {
        Ok(phase) => phase,
        Err(terminal) => return settle_terminal(terminal, &name),
    };
    settle_terminal(staged.commit().await, &name)
}

fn settle_terminal(terminal: TerminalPhase, name: &ProfileName) -> Result<String, GatewayError> {
    let report = terminal.finish()?;
    #[cfg(feature = "local")]
    if !report.failed.is_empty() {
        return Err(GatewayError::PartialStart {
            profile: name.to_string(),
            loaded: report.loaded,
            failed: report.failed,
        });
    }
    #[cfg(not(feature = "local"))]
    let StartReport {} = report;

    tracing::info!(profile = %name, "switched profile");
    Ok(name.to_string())
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
        speech,
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

#[cfg(all(test, feature = "stt"))]
/// Resolves only a target for tests of the unchanged terminal commit.
pub(super) async fn prepare_target_for_test(
    state: &AppState,
    name: &ProfileName,
    tree: &ProgressTree,
    candidate: Option<Config>,
) -> Result<StagedTarget, GatewayError> {
    let SwitchTarget {
        config,
        remote_routing,
        #[cfg(feature = "web-search")]
        web_search,
        allowlist,
        loading: _,
        speech: _,
    } = prepare_target(state, name, tree, candidate).await?;
    Ok(StagedTarget {
        config,
        remote_routing,
        #[cfg(feature = "web-search")]
        web_search,
        allowlist,
    })
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
        #[cfg(any(test, not(feature = "local")))]
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

#[cfg(feature = "local")]
fn restart_local_runtime(
    _state: &AppState,
    config: &Config,
) -> Result<LocalRuntime, crate::local::LocalError> {
    #[cfg(test)]
    if let Some(restarter) = _state.local_restarter {
        return restarter(config);
    }
    LocalRuntime::start(config, None)
}

async fn restore_runtime_snapshot(
    state: &AppState,
    prior: PriorRuntimeSnapshot,
) -> Result<(), GatewayError> {
    #[cfg(feature = "local")]
    let local = if prior.restart_local {
        let config = Arc::clone(&prior.config);
        let restart_state = state.clone();
        tokio::time::timeout(
            STAGE_TIMEOUT,
            tokio::task::spawn_blocking(move || restart_local_runtime(&restart_state, &config)),
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
    #[cfg(feature = "local")]
    let routing = if prior.routing_was_empty {
        Routing::empty()
    } else {
        Routing::from_config(&prior.config)
            .map_err(|error| GatewayError::switch_failed("rollback-routing", error))?
            .merge(local.models().iter().cloned())
            .map_err(|error| GatewayError::switch_failed("rollback-routing", error))?
    };
    let mut live = state.live.write().await;
    if !prior.routing_was_empty {
        #[cfg(feature = "local")]
        {
            live.routing = Arc::new(routing);
        }
        #[cfg(not(feature = "local"))]
        {
            live.routing = prior.routing;
        }
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

pub(super) fn request_fatal_shutdown(
    state: &AppState,
    token: &CancellationToken,
    phase: &'static str,
    error: GatewayError,
) -> GatewayError {
    token.cancel();
    state.shutdown.fire();
    #[cfg(feature = "stt")]
    state.speech.shutdown();
    GatewayError::switch_failed(phase, error)
}

#[cfg_attr(
    not(feature = "stt"),
    expect(
        unused_variables,
        reason = "featureless runtime replacement has no speech owner to restore"
    )
)]
fn rollback_runtime(state: &AppState, replacement: RuntimeReplacement) -> Result<(), GatewayError> {
    #[cfg(feature = "stt")]
    state
        .speech
        .abort_replacement(replacement.speech)
        .map_err(|error| GatewayError::switch_failed("rollback-stt", error))?;
    #[cfg(not(feature = "stt"))]
    let _replacement = replacement;
    Ok(())
}

struct RollbackOwner {
    state: AppState,
    prior: PriorRuntimeSnapshot,
    token: CancellationToken,
    failure: GatewayError,
    runtime_rollback: Result<(), GatewayError>,
}

impl RollbackOwner {
    async fn finish(self) -> TerminalPhase {
        match self.runtime_rollback {
            Ok(()) => match restore_runtime_snapshot(&self.state, self.prior).await {
                Ok(()) => TerminalPhase::RolledBack(RolledBackPhase {
                    error: self.failure,
                }),
                Err(rollback) => TerminalPhase::Indeterminate(IndeterminatePhase {
                    error: request_fatal_shutdown(
                        &self.state,
                        &self.token,
                        "rollback-profile",
                        rollback,
                    ),
                }),
            },
            Err(rollback) => {
                let failure = GatewayError::switch_failed(
                    "determinate-profile-failure",
                    std::io::Error::other(format!(
                        "{}; {}",
                        crate::config_write::error_chain(&self.failure),
                        crate::config_write::error_chain(&rollback)
                    )),
                );
                TerminalPhase::Indeterminate(IndeterminatePhase {
                    error: request_fatal_shutdown(
                        &self.state,
                        &self.token,
                        "rollback-staged-profile",
                        failure,
                    ),
                })
            }
        }
    }
}

#[cfg(all(test, feature = "stt"))]
pub(super) async fn commit_for_test(
    state: &AppState,
    name: ProfileName,
    target: StagedTarget,
    replacement: RuntimeReplacement,
    persistence: PreparedPersistence,
    token: CancellationToken,
) -> Result<StartReport, GatewayError> {
    let prior = capture_runtime_snapshot(state).await;
    StagedPhase {
        state: state.clone(),
        name,
        target,
        replacement,
        persistence,
        prior,
        token,
    }
    .commit()
    .await
    .finish()
}

#[cfg(feature = "local")]
fn start_report(replacement: &RuntimeReplacement) -> StartReport {
    StartReport {
        loaded: replacement
            .local
            .models()
            .iter()
            .map(|model| model.name.clone())
            .collect(),
        failed: replacement
            .start_failures
            .iter()
            .map(|failure| format!("{}: {}", failure.model(), failure.error()))
            .collect(),
    }
}

#[cfg(not(feature = "local"))]
fn start_report(_replacement: &RuntimeReplacement) -> StartReport {
    StartReport {}
}

#[cfg(feature = "stt")]
pub(super) fn classify_speech_stage_failure(
    error: gateway_stt::SpeechError,
) -> RuntimeStageFailure {
    let indeterminate = error.is_non_preemptible_startup_timeout();
    let error = GatewayError::switch_failed("start-stt", error);
    if indeterminate {
        RuntimeStageFailure::Indeterminate(error)
    } else {
        RuntimeStageFailure::Determinate(error)
    }
}

#[cfg(not(any(feature = "local", feature = "stt")))]
async fn spawn_runtimes(
    _config: &Config,
    _tree: &ProgressTree,
    _token: &CancellationToken,
    _deadline: std::time::Instant,
) -> Result<RuntimeReplacement, RuntimeStageFailure> {
    Ok(RuntimeReplacement {})
}

#[cfg(any(feature = "local", feature = "stt"))]
#[expect(
    clippy::too_many_lines,
    reason = "the moved staging sequence preserves one shared deadline and exact local-before-speech cancellation order"
)]
async fn spawn_runtimes(
    config: &Config,
    #[cfg(feature = "stt")] speech: SpeechService,
    #[cfg(feature = "stt")] prepared_speech: gateway_stt::PreparedSpeech,
    tree: &ProgressTree,
    token: &CancellationToken,
    deadline: std::time::Instant,
) -> Result<RuntimeReplacement, RuntimeStageFailure> {
    let starting = tree.register("starting-models", 5.0);
    #[cfg(feature = "local")]
    let start_config = config.clone();
    #[cfg(feature = "local")]
    let start_progress = starting.clone();
    #[cfg(feature = "local")]
    let outcome = {
        let start_token = token.clone();
        let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let bridge = tokio::spawn({
            let interrupted = Arc::clone(&interrupted);
            let token = token.clone();
            async move {
                token.cancelled().await;
                interrupted.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        let result = tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            tokio::task::spawn_blocking(move || {
                LocalRuntime::start_partial_with_cancellation(
                    &start_config,
                    Some(&start_progress),
                    &start_token,
                    &interrupted,
                )
            }),
        )
        .await;
        bridge.abort();
        match result {
            Ok(Ok(Ok(outcome))) => outcome,
            Ok(Ok(Err(error))) => {
                starting.fail();
                return Err(RuntimeStageFailure::Determinate(
                    GatewayError::switch_failed("start-local", error),
                ));
            }
            Ok(Err(error)) => {
                starting.fail();
                return Err(RuntimeStageFailure::Determinate(
                    GatewayError::switch_failed("start-local-task", error),
                ));
            }
            Err(_) => {
                starting.fail();
                return Err(RuntimeStageFailure::Indeterminate(
                    GatewayError::switch_failed(
                        "start-local-timeout",
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "local runtime startup exceeded the shared profile deadline",
                        ),
                    ),
                ));
            }
        }
    };
    #[cfg(feature = "local")]
    let (runtime, failures) = outcome.into_parts();
    #[cfg(feature = "stt")]
    if token.is_cancelled() {
        return Err(RuntimeStageFailure::Determinate(
            GatewayError::CommandCancelled("profile switch".to_owned()),
        ));
    }
    #[cfg(feature = "stt")]
    let speech = match tokio::task::spawn_blocking(move || {
        speech.begin_replacement_before(prepared_speech, deadline)
    })
    .await
    {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(error)) => {
            starting.fail();
            return Err(classify_speech_stage_failure(error));
        }
        Err(error) => {
            starting.fail();
            return Err(RuntimeStageFailure::Determinate(
                GatewayError::switch_failed("start-stt-task", error),
            ));
        }
    };
    #[cfg(feature = "local")]
    if failures.is_empty() {
        starting.complete();
    } else {
        starting.fail();
    }
    #[cfg(not(feature = "local"))]
    starting.complete();
    Ok(RuntimeReplacement {
        #[cfg(feature = "local")]
        local: runtime,
        #[cfg(feature = "local")]
        start_failures: failures,
        #[cfg(feature = "stt")]
        speech,
    })
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
