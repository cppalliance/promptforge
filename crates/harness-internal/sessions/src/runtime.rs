//! The Harness handle: its configuration, the Host's run recorder,
//! capability registry, and services, the bindings a client pushes
//! through the public API, and the sessions it serves.
//!
//! One [`Harness`] serves every session a client launches. The client
//! holds it behind an `Arc`, pushes the gateway binding at startup and on
//! every replacement (the capability registry, the Host's plus the
//! built-in web, and the model client are rebuilt when the generation
//! changes), pushes its chat catalog and Host snapshot as they change,
//! and launches sessions by discovered agent name. Sessions outlive
//! client connections: a client that reattaches looks its session up by
//! id and reads the transcript past its cursor.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use harness_capabilities::{CapabilityRegistry, HostServices};
use harness_runner::recorder::RunRecorder;
use harness_runner::spawn::{spawn_blocking_launch, spawn_session};
use promptforge::vfs::VfsRef;
use tokio::sync::mpsc;

use crate::discovery::{agent_source, discover_agents};
use crate::environment::{Bindings, CatalogBinding, GatewayBinding, HostSnapshot};
use crate::lifecycle::{CANCELLATION_CAPACITY, RunLifecycle};
use crate::protocol::{LaunchRequest, SessionId};
use crate::session::files::SessionFiles;
use crate::session::supervisor::{Supervisor, SupervisorParts};
use crate::session::{Session, SessionCore, SessionSeed};

/// What a client tells the Harness at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConfig {
    /// The directory the Harness discovers launchable agents in.
    pub agents_path: PathBuf,
}

/// What a launch hands the session beyond its [`LaunchRequest`]: the
/// session's environment rather than data about what to run. Build it
/// with `..LaunchOptions::default()` so a later field is not a break.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// The filesystem every run of the session works in: its declared
    /// store, and any real mounts, overlays, policy, and op sink the
    /// client built into the handle with `promptforge::vfs`. Every run
    /// shares it, relaunches included, so files a retired run wrote are
    /// still there, and the declared input is staged and the declared
    /// output read through its store. The Harness owns the handle from
    /// launch on; a backend, policy, or op sink in it runs inside the
    /// session's store operations. `None` gives each run a fresh memory
    /// store at `/`.
    pub vfs: Option<VfsRef>,
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

/// The Harness, which steps every Engine run and performs its effects,
/// seen from outside the family.
pub struct Harness {
    config: HarnessConfig,
    bindings: Arc<Bindings>,
    /// The Host's recorder: every run of every session writes to it.
    recorder: Arc<dyn RunRecorder>,
    /// The Host's installed capabilities: every gateway generation's
    /// registry, which runs resolve against, starts from them.
    capabilities: CapabilityRegistry,
    /// The Host's services: every run's capabilities read them.
    services: HostServices,
    sessions: Arc<SessionTable>,
}

impl fmt::Debug for Harness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Harness")
            .field("config", &self.config)
            .field("bindings", &self.bindings)
            .field("capabilities", &self.capabilities)
            .field("services", &self.services)
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

impl Harness {
    /// A Harness over `config` with no gateway bound yet, which records
    /// every run it makes through `recorder`, resolves every run's
    /// declared capabilities against `capabilities`, plus the built-in
    /// `promptforge/web` when `capabilities` holds none and the gateway
    /// builds it, and hands `services` to the capabilities it activates.
    /// Nothing touches the filesystem
    /// here, and the Harness opens no file at launch either: discovery
    /// reads the agents directory per request, and the recorder is the
    /// Host's own.
    #[must_use]
    pub fn new(
        config: HarnessConfig,
        recorder: Arc<dyn RunRecorder>,
        capabilities: CapabilityRegistry,
        services: HostServices,
    ) -> Self {
        Self {
            config,
            bindings: Arc::new(Bindings::new()),
            recorder,
            capabilities,
            services,
            sessions: Arc::new(SessionTable::default()),
        }
    }

    /// The configuration this Harness was built with.
    #[must_use]
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    /// Replaces the gateway binding; the latest call wins. The capability
    /// registry and model client are rebuilt when `binding.generation`
    /// differs from the current one, and every session observes the new
    /// generation.
    pub fn set_gateway(&self, binding: GatewayBinding) {
        self.bindings.set_gateway(binding, &self.capabilities);
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

    /// Replaces the Host snapshot; the next launch reads it.
    pub fn set_host(&self, host: HostSnapshot) {
        self.bindings.set_host(host);
    }

    /// The launchable agent names: the `.md` file stems under the
    /// configured agents directory plus the built-in `chat`, sorted.
    #[must_use]
    pub fn discover(&self) -> Vec<String> {
        discover_agents(&self.config.agents_path)
    }

    /// Launches a session running the discovered agent `request.agent`
    /// with `request.args`, staging `request.input_text` at the prompt's
    /// declared input file, and returns it. The session runs until its
    /// program returns, fails, or it is closed; turn-cancel relaunches the
    /// program over the retained transcript without ending the session.
    /// Each run works in a fresh memory store; [`Harness::launch_with`]
    /// hands the session a filesystem of the client's own.
    ///
    /// # Errors
    /// Returns [`LaunchError::UnknownAgent`] when the name is not a
    /// discovered agent (which also refuses names that look like paths:
    /// discovery yields bare file stems), [`LaunchError::GatewayUnusable`]
    /// when no usable gateway is bound, and [`LaunchError::SessionState`]
    /// when the agent's source cannot be read. A recorder that refuses a
    /// write never refuses the launch: it fails the run, which the session
    /// reports.
    pub async fn launch(&self, request: LaunchRequest) -> Result<Session, LaunchError> {
        self.launch_with(request, LaunchOptions::default()).await
    }

    /// Launches a session as [`Harness::launch`] does, under `options`:
    /// every run of the session works in `options.vfs` when it is set.
    ///
    /// # Errors
    /// Returns the errors [`Harness::launch`] does.
    pub async fn launch_with(
        &self,
        request: LaunchRequest,
        options: LaunchOptions,
    ) -> Result<Session, LaunchError> {
        let LaunchRequest {
            agent,
            args,
            input_text,
        } = request;
        let LaunchOptions { vfs } = options;
        // Resolving through the discovered list is the trust boundary: a
        // client-sent name never reaches the filesystem unless it is the
        // bare stem of a real `.md` file in the configured directory. The
        // directory walk is filesystem work and runs on the blocking pool,
        // through the Harness's one spawn site.
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

        let (events, lifecycle_rx) = mpsc::unbounded_channel();
        let (cancellations, cancellations_rx) = mpsc::channel(CANCELLATION_CAPACITY);
        let id = SessionId::fresh();
        let (core, raw_deltas) = SessionCore::new(SessionSeed {
            id: id.clone(),
            prompt_path: self.config.agents_path.join(format!("{agent}.md")),
            agent,
            source,
            args,
            files: SessionFiles::new(vfs, input_text),
            lifecycle: Arc::new(RunLifecycle::new(events, cancellations)),
            recorder: Arc::clone(&self.recorder),
            services: self.services.clone(),
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
    /// the Harness at once. Returns whether a session was ended. The
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
