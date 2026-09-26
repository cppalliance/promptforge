//! The harness handle: its configuration, the bindings a client pushes
//! through the public API, the run log, and the sessions it serves.
//!
//! One [`Harness`] serves every session a client launches. The client
//! holds it behind an `Arc`, pushes the gateway binding at startup and on
//! every replacement (the capability registry and model client are
//! rebuilt when the generation changes), pushes its chat catalog and host
//! snapshot as they change, and launches sessions by discovered agent
//! name. Sessions outlive client connections: a client that reattaches
//! looks its session up by id and reads the transcript past its cursor.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use harness_log::{LogError, RunLog};
use harness_runner::effect_loop::SharedLog;
use harness_runner::spawn::{spawn_blocking_launch, spawn_session};
use tokio::sync::{OnceCell, mpsc};

use crate::discovery::{agent_source, discover_agents};
use crate::environment::{Bindings, CatalogBinding, GatewayBinding, HostSnapshot};
use crate::lifecycle::{CANCELLATION_CAPACITY, RunLifecycle};
use crate::protocol::{LaunchRequest, SessionId};
use crate::session::supervisor::{Supervisor, SupervisorParts};
use crate::session::{Session, SessionCore, SessionSeed};

/// The file under the state directory the run log is stored in.
const RUN_LOG_FILE: &str = "runs.db";

/// What a client tells the harness at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConfig {
    /// The directory the harness discovers launchable agents in.
    pub agents_path: PathBuf,
    /// The directory the harness keeps its state under, the run log
    /// included.
    pub state_dir: PathBuf,
}

/// A refused launch.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LaunchError {
    /// The requested name is not a discovered agent.
    #[error("unknown agent {name:?}: not in the agents directory")]
    UnknownAgent {
        /// The name that was requested.
        name: String,
    },
    /// No gateway is bound, or the bound gateway's settings could not
    /// make a model client, so no agent could complete a model round.
    #[error("agent sessions need a usable gateway binding; check the gateway base URL and key")]
    GatewayUnusable,
    /// The agent's program source could not be read.
    #[error("agent session state unavailable")]
    SessionState {
        /// The underlying filesystem failure.
        #[source]
        source: io::Error,
    },
    /// The run log could not be opened or written.
    #[error(transparent)]
    Log(#[from] LogError),
}

/// The running sessions by id, shared with each supervisor so a finished
/// session removes itself.
#[derive(Default)]
pub(crate) struct SessionTable {
    sessions: Mutex<HashMap<SessionId, Arc<SessionCore>>>,
}

impl SessionTable {
    fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<SessionCore>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn insert(&self, core: Arc<SessionCore>) {
        self.lock().insert(core.id.clone(), core);
    }

    fn get(&self, id: &SessionId) -> Option<Arc<SessionCore>> {
        self.lock().get(id).cloned()
    }

    fn remove(&self, id: &SessionId) -> Option<Arc<SessionCore>> {
        self.lock().remove(id)
    }

    /// Removes a finished session, unless a close already did.
    pub(crate) fn forget(&self, id: &SessionId) {
        self.lock().remove(id);
    }

    fn len(&self) -> usize {
        self.lock().len()
    }
}

/// The harness: the engine's production host, seen from outside the
/// family.
pub struct Harness {
    config: HarnessConfig,
    bindings: Arc<Bindings>,
    /// Opened on the first launch; a failed open is retried by the next.
    log: OnceCell<SharedLog>,
    sessions: Arc<SessionTable>,
}

impl fmt::Debug for Harness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Harness")
            .field("config", &self.config)
            .field("bindings", &self.bindings)
            .field("log", &self.log.initialized())
            .field("sessions", &self.sessions.len())
            .finish()
    }
}

impl Harness {
    /// A harness over `config` with no gateway bound yet. Nothing touches
    /// the filesystem here: discovery reads the agents directory per
    /// request, and the run log opens on the first launch.
    #[must_use]
    pub fn new(config: HarnessConfig) -> Self {
        Self {
            config,
            bindings: Arc::new(Bindings::new()),
            log: OnceCell::new(),
            sessions: Arc::new(SessionTable::default()),
        }
    }

    /// The configuration this harness was built with.
    #[must_use]
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    /// Replaces the gateway binding; the latest call wins. The capability
    /// registry and model client are rebuilt when `binding.generation`
    /// differs from the current one, and every session observes the new
    /// generation.
    pub fn set_gateway(&self, binding: GatewayBinding) {
        self.bindings.set_gateway(binding);
    }

    /// The most recently set gateway binding, or `None` before the first
    /// [`Harness::set_gateway`].
    #[must_use]
    pub fn gateway(&self) -> Option<GatewayBinding> {
        self.bindings
            .gateway()
            .map(|resources| resources.binding().clone())
    }

    /// Replaces the chat catalog binding; every session observes the new
    /// generation and retires its run when the models changed.
    pub fn set_catalog(&self, catalog: CatalogBinding) {
        self.bindings.set_catalog(catalog);
    }

    /// Replaces the host snapshot; the next launch reads it.
    pub fn set_host(&self, host: HostSnapshot) {
        self.bindings.set_host(host);
    }

    /// The launchable agent names: the `.md` file stems under the
    /// configured agents directory plus the built-in `chat`, sorted.
    #[must_use]
    pub fn discover(&self) -> Vec<String> {
        discover_agents(&self.config.agents_path)
    }

    /// The run log, opened under the state directory on first use; the
    /// test-only way a suite reads back what the sessions recorded.
    ///
    /// # Errors
    /// Returns the log's error when the state directory cannot be created
    /// or the log cannot be opened.
    #[cfg(feature = "test-support")]
    pub async fn log(&self) -> Result<SharedLog, LogError> {
        self.run_log().await
    }

    /// The run log, opened under the state directory on first use.
    pub(crate) async fn run_log(&self) -> Result<SharedLog, LogError> {
        self.log
            .get_or_try_init(|| async {
                tokio::fs::create_dir_all(&self.config.state_dir).await?;
                let log = RunLog::open(&self.config.state_dir.join(RUN_LOG_FILE)).await?;
                Ok(Arc::new(tokio::sync::Mutex::new(log)))
            })
            .await
            .cloned()
    }

    /// Launches a session running the discovered agent `request.agent`
    /// with `request.args` and returns it. The session runs until its
    /// program returns, fails, or it is closed; turn-cancel relaunches the
    /// program over the retained transcript without ending the session.
    ///
    /// # Errors
    /// Returns [`LaunchError::UnknownAgent`] when the name is not a
    /// discovered agent (which also refuses names that look like paths:
    /// discovery yields bare file stems), [`LaunchError::GatewayUnusable`]
    /// when no usable gateway is bound, [`LaunchError::SessionState`] when
    /// the agent's source cannot be read, and [`LaunchError::Log`] when the
    /// run log cannot be opened.
    pub async fn launch(&self, request: LaunchRequest) -> Result<Session, LaunchError> {
        let LaunchRequest { agent, args } = request;
        // Resolving through the discovered list is the trust boundary: a
        // client-sent name never reaches the filesystem unless it is the
        // bare stem of a real `.md` file in the configured directory. The
        // directory walk is filesystem work and runs on the blocking pool,
        // through the harness's one spawn site.
        let agents_path = self.config.agents_path.clone();
        let known = spawn_blocking_launch(&agent, move || discover_agents(&agents_path))
            .await
            .map_err(|join| LaunchError::SessionState {
                source: io::Error::other(join),
            })?;
        if !known.contains(&agent) {
            return Err(LaunchError::UnknownAgent { name: agent });
        }
        // Subscribe before reading the snapshot: `watch::Sender::subscribe`
        // marks every earlier send as seen, so a replacement landing
        // between the two calls would otherwise never wake the supervisor
        // and the session would stay on the stale generation.
        let gateway_watch = self.bindings.subscribe_gateway();
        // The client is checked at launch, not at startup: a client whose
        // gateway settings cannot make a model client still runs, but an
        // agent run would fail its first model round, so the launch
        // refuses instead.
        let gateway = self
            .bindings
            .gateway()
            .filter(|resources| resources.client().is_some())
            .ok_or(LaunchError::GatewayUnusable)?;
        // The source read is filesystem work too; a worker that cannot
        // report is the same unavailable state as an unreadable file.
        let agents_path = self.config.agents_path.clone();
        let name = agent.clone();
        let source = spawn_blocking_launch(&agent, move || agent_source(&agents_path, &name))
            .await
            .unwrap_or_else(|join| Err(io::Error::other(join)))
            .map_err(|source| LaunchError::SessionState { source })?;
        let log = self.run_log().await?;

        let (events, lifecycle_rx) = mpsc::unbounded_channel();
        let (cancellations, cancellations_rx) = mpsc::channel(CANCELLATION_CAPACITY);
        let id = SessionId::fresh();
        let (core, raw_deltas) = SessionCore::new(SessionSeed {
            id: id.clone(),
            prompt_path: self.config.agents_path.join(format!("{agent}.md")),
            agent,
            source,
            args,
            lifecycle: Arc::new(RunLifecycle::new(events, cancellations)),
            log,
        });
        self.sessions.insert(Arc::clone(&core));
        let supervisor = Supervisor::new(SupervisorParts {
            core: Arc::clone(&core),
            bindings: Arc::clone(&self.bindings),
            table: Arc::clone(&self.sessions),
            lifecycle: lifecycle_rx,
            cancellations: cancellations_rx,
            raw_deltas,
            gateway,
            gateway_watch,
        });
        spawn_session(id.as_str(), supervisor.run());
        Ok(Session::new(core))
    }

    /// The running session with this id, when one exists: how a client
    /// reattaches after a disconnect.
    #[must_use]
    pub fn session(&self, id: &SessionId) -> Option<Session> {
        self.sessions.get(id).map(Session::new)
    }

    /// Ends the session with this id: its run is cancelled for good (no
    /// relaunch), pending waits die as cancelled, and the session leaves
    /// the harness at once. Returns whether a session was ended. The
    /// run's outstanding effects are answered `Dropped` before its state
    /// reaches `Closed`; a handle still held sees that through
    /// [`Session::subscribe_state`].
    #[must_use]
    pub fn close(&self, id: &SessionId) -> bool {
        let Some(core) = self.sessions.remove(id) else {
            return false;
        };
        Session::new(core).close();
        true
    }
}
