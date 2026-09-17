//! Agent sessions: discovery of `.md` agent programs, the
//! [`AgentSessions`] registry, and each session's run lifecycle.
//!
//! A session owns one running agent: its persisting event log
//! ([`workshop_gateway::WorkshopObserver`], JSONL under
//! `state_dir/sessions/<session-id>.jsonl`), its
//! [`crate::input::WaitRegistry`] and `user_input` tool, its dedicated
//! delta broadcast (deltas never enter the event log), and the retained
//! cancel handle behind turn-cancel. The supervisor task relaunches
//! the agent run over the retained event log after a turn-cancel -
//! cancellation is a stop reason, never an error - and ends the session
//! when the program returns or fails.
//!
//! **Registry carve-out.** Sessions survive socket disconnect and sockets
//! attach and detach (`socket`), so this module keeps the session
//! registry the crate's socket rule otherwise forbids. The rule governed
//! per-request relay work, where every held resource belonged to one
//! socket; an agent session is longer-lived than any socket on purpose,
//! and the registry is the one place that owns it.
//!
//! Reply ids coalesce deltas: every live delta is stamped with the id of
//! the durable event that will supersede it. The id is the count of
//! settled model rounds - the session observer advances it as the reply
//! or tool-call event lands, before the program
//! resumes, and the socket derives the same count from the event sequence
//! itself, so both sides agree without sharing more than the log.

mod environment;
mod lifecycle;
mod session;
pub(crate) mod socket;
mod supervisor;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{broadcast, mpsc};

use promptforge_api_runtime::client::{GatewayClient, GatewayEndpoint, SecretString};
use workshop_gateway::{GatewayBinding, WorkshopObserver};
use workshop_menu::{CatalogBus, MenuBus};
use workshop_registry::{Push, Registry};
use workshop_support::ReconnectBackoff;

use crate::input::WaitRegistry;

use self::lifecycle::RunLifecycle;

pub use environment::session_environment;
pub(crate) use session::{AgentDelta, AgentSession, AgentSource, SessionObserver};
pub(crate) use session::{delta_stamp, reply_stamp, ui_provider};

/// Capacity of a session's delta broadcast. Deltas are ephemeral: a
/// receiver that lags loses chunks, and the completed-reply event is the
/// repair path.
pub(crate) const DELTA_CAPACITY: usize = 256;

/// Capacity of a session's input-frame broadcast. A session holds at
/// most a handful of waits; the registry's retained state is the
/// durable-delivery repair path on lag.
const INPUT_CAPACITY: usize = 32;

/// Capacity of a session's error broadcast. Session errors are rare
/// one-off reports: a failed model round or a run that ended in error
/// surfaces one frame each, and a receiver that lags misses only what
/// the durable transcript already shows as a turn without a reply.
pub(crate) const ERROR_CAPACITY: usize = 8;

/// The committed built-in chat agent, embedded at compile time - the same
/// shipped-asset pattern as the SPA `dist/` - so a fresh install has a
/// working chat with no agents directory at all. The built-in is a
/// Markdown prompt on the unified runtime.
pub(crate) const BUILTIN_CHAT_SOURCE: &str = include_str!("../agents/chat.md");

/// The built-in default agent's name: discovery always offers it, and a
/// directory file named `chat.md` shadows the embedded source.
const BUILTIN_CHAT_NAME: &str = "chat";

/// The shared handles a session's lifecycle reports flow through,
/// captured once at [`AgentSessions`] construction. The buses come from
/// the menu subsystem; the push facade and the workspace-roots handle
/// are read through the subsystem registry's slots, so this host never
/// names the workspace crate the tier graph forbids.
#[derive(Debug, Clone)]
pub struct SessionHost {
    /// The subsystem registry: the push facade and the workspace-roots
    /// slot are read through it.
    registry: Registry,
    /// Reset on completed replies: an agent reply is useful gateway work.
    backoff: ReconnectBackoff,
    /// Serves `selected_model` to the agent's `ui()` snapshot.
    menu: MenuBus,
    /// The retained gateway catalog the session's model catalog is built
    /// from at launch.
    catalog: CatalogBus,
}

impl SessionHost {
    /// Bundles the registry and the bus handles for one sessions host.
    #[must_use]
    pub fn new(
        registry: Registry,
        backoff: ReconnectBackoff,
        menu: MenuBus,
        catalog: CatalogBus,
    ) -> Self {
        Self {
            registry,
            backoff,
            menu,
            catalog,
        }
    }

    /// The push facade over the registry's producer sink slots.
    pub(crate) fn push(&self) -> Push {
        self.registry.push()
    }

    /// The subsystem registry the `ui()` snapshot reads the workspace
    /// roots slot through.
    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The shared reconnect backoff, reset on completed replies.
    pub(crate) fn backoff(&self) -> &ReconnectBackoff {
        &self.backoff
    }

    /// The menu bus serving `selected_model` to the `ui()` snapshot.
    pub(crate) fn menu(&self) -> &MenuBus {
        &self.menu
    }

    /// The retained catalog the session's model catalog is built from.
    pub(crate) fn catalog(&self) -> &CatalogBus {
        &self.catalog
    }
}

/// The registry of running agent sessions.
///
/// Typed and construction-phased: everything a launch needs is captured
/// when the composition root builds it, and the only mutable state
/// is the session map itself. Sessions survive socket disconnect -
/// sockets attach and detach through the `socket` module - which is this
/// module's documented carve-out from the crate's no-session-registry
/// socket rule.
#[derive(Clone)]
pub struct AgentSessions {
    inner: Arc<Inner>,
}

/// The shared registry state behind the cloneable handle.
struct Inner {
    /// Directory whose `.md` files are the launchable agents.
    agents_dir: PathBuf,
    /// Where session event JSONLs persist (`state_dir/sessions`).
    sessions_dir: PathBuf,
    /// The atomically replaceable Gateway clients every run snapshots.
    gateway: GatewayBinding,
    /// The shared handles session lifecycles report through.
    host: SessionHost,
    /// The running sessions by id.
    sessions: Mutex<HashMap<String, Arc<AgentSession>>>,
}

impl fmt::Debug for AgentSessions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSessions")
            .field("agents_dir", &self.inner.agents_dir)
            .field("sessions", &self.lock().len())
            .finish_non_exhaustive()
    }
}

impl AgentSessions {
    /// Builds the registry over the discovery directory, the sessions
    /// state directory, the model client agents complete through, and
    /// the shared handles. Nothing touches the filesystem here:
    /// discovery reads the agents directory per request, and the
    /// sessions directory is created at first launch.
    #[must_use]
    pub fn new(
        agents_dir: PathBuf,
        sessions_dir: PathBuf,
        gateway: GatewayBinding,
        host: SessionHost,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                agents_dir,
                sessions_dir,
                gateway,
                host,
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The launchable agent names: the `.md` file stems under the
    /// configured agents directory plus the built-in `chat`, sorted. The
    /// built-in is always offered - a missing or unreadable directory
    /// still lists it, so a fresh install always has a working chat - and
    /// a directory file named `chat.md` shadows the embedded source
    /// rather than listing twice.
    #[must_use]
    pub fn discover(&self) -> Vec<String> {
        discover_agents(&self.inner.agents_dir)
    }

    /// Launches a session running the discovered agent `name` and
    /// returns it. The session runs until its program returns, fails, or
    /// [`close`](Self::close) ends it; turn-cancel relaunches the program
    /// over the retained event log without ending the session.
    ///
    /// # Errors
    /// Returns [`LaunchRefusal::UnknownAgent`] when `name` is not a
    /// discovered agent (which also refuses path-shaped names: discovery
    /// yields bare file stems), [`LaunchRefusal::GatewayUnusable`] when
    /// the workshop gateway settings could not make a model client, and
    /// [`LaunchRefusal::SessionState`] when the sessions directory or the
    /// session's event log cannot be created.
    pub(crate) fn launch(&self, name: &str) -> Result<Arc<AgentSession>, LaunchRefusal> {
        // Resolving through the discovered list is the trust boundary: a
        // client-sent name never reaches the filesystem unless it is the
        // bare stem of a real `.md` file in the configured directory.
        if !self.discover().iter().any(|agent| agent == name) {
            return Err(LaunchRefusal::UnknownAgent {
                name: name.to_owned(),
            });
        }
        // The client is checked at launch, not at startup: a workshop
        // whose gateway settings cannot make a model client still serves
        // chat, but an agent run would fail its first model round - or
        // silently resolve a different gateway from the environment - so
        // the launch refuses instead.
        let snapshot = self.inner.gateway.snapshot();
        if agent_client(snapshot.base_url(), snapshot.api_key()).is_none() {
            return Err(LaunchRefusal::GatewayUnusable);
        }
        let source = agent_source(&self.inner.agents_dir, name)
            .map_err(|source| LaunchRefusal::SessionState { source })?;
        std::fs::create_dir_all(&self.inner.sessions_dir)
            .map_err(|source| LaunchRefusal::SessionState { source })?;
        let id = fresh_session_id();
        let log_path = self.inner.sessions_dir.join(format!("{id}.jsonl"));
        let observer = Arc::new(
            WorkshopObserver::new(Some(&log_path))
                .map_err(|source| LaunchRefusal::SessionState { source })?,
        );
        let (supervisor_events, events) = mpsc::unbounded_channel();
        let (cancellations, cancellation_events) = mpsc::channel(lifecycle::CANCELLATION_CAPACITY);
        let lifecycle = Arc::new(RunLifecycle::new(supervisor_events, cancellations));
        let waits = Arc::new(WaitRegistry::new());
        let (input_frames, _) = broadcast::channel(INPUT_CAPACITY);
        let (deltas, _) = broadcast::channel(DELTA_CAPACITY);
        let (errors, _) = broadcast::channel(ERROR_CAPACITY);
        let session = Arc::new(AgentSession::new(
            id.clone(),
            name,
            source,
            observer,
            lifecycle,
            waits,
            input_frames,
            deltas,
            errors,
        ));
        self.lock().insert(id, Arc::clone(&session));
        supervisor::spawn(
            Arc::clone(&session),
            self.clone(),
            self.inner.host.clone(),
            self.inner.gateway.clone(),
            events,
            cancellation_events,
        );
        Ok(session)
    }

    /// The running session with this id, when one exists.
    pub(crate) fn get(&self, id: &str) -> Option<Arc<AgentSession>> {
        self.lock().get(id).cloned()
    }

    /// Ends the session with this id: its run is cancelled for good (no
    /// relaunch), pending waits die as `input_cancelled`, and the session
    /// leaves the registry. Returns whether a session was ended. The
    /// persisted event JSONL stays on disk.
    #[must_use]
    pub fn close(&self, id: &str) -> bool {
        let Some(session) = self.lock().remove(id) else {
            return false;
        };
        session.close();
        true
    }

    /// The unresolved wait tokens of the session with this id - the
    /// teardown leak probe: after a close or a finished run, the list
    /// must be empty. `None` when no such session is registered.
    #[must_use]
    pub fn unresolved_waits(&self, id: &str) -> Option<Vec<String>> {
        Some(self.get(id)?.waits.unresolved())
    }

    /// Delivers a fixture response after running `after_acceptance`
    /// between its durable observation and the waiting tool's resumption.
    #[cfg(feature = "test-fixtures")]
    pub fn deliver_input_after_acceptance_for_test(
        &self,
        id: &str,
        response: workshop_protocol::InputResponse,
        after_acceptance: impl FnOnce(),
    ) -> Option<Result<(), crate::input::WaitError>> {
        let session = self.get(id)?;
        Some(session.accept_input(response, after_acceptance))
    }

    /// The session map guard; a lock poisoned by a panicking peer
    /// recovers the value rather than wedging the process (zone two).
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<AgentSession>>> {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Removes a finished session from the map, unless a close already
    /// did.
    fn forget(&self, id: &str) {
        self.lock().remove(id);
    }
}

/// A refused agent launch, relayed to the client as an error frame.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum LaunchRefusal {
    /// The requested name is not a discovered agent.
    #[error("unknown agent {name:?}: not in the agents directory")]
    UnknownAgent {
        /// The name that was requested.
        name: String,
    },
    /// The workshop gateway settings could not make a model client, so
    /// no agent could complete a model round.
    #[error(
        "agent sessions need a usable gateway client; check `gateway.base_url` and \
         `gateway.api_key` in workshop.toml"
    )]
    GatewayUnusable,
    /// The session's on-disk state could not be prepared.
    #[error("agent session state unavailable")]
    SessionState {
        /// The underlying filesystem failure.
        #[source]
        source: io::Error,
    },
}

/// Builds the agent completion client from one Gateway snapshot's base
/// URL and bearer, through the `promptforge-api-runtime` client re-exports.
/// `None` - reported as [`LaunchRefusal::GatewayUnusable`] at launch and
/// as a failed relaunch by the supervisor - when the key or URL cannot
/// build a client.
pub(crate) fn agent_client(base_url: &str, api_key: &str) -> Option<GatewayClient> {
    let key = match SecretString::new(api_key) {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway API key unusable");
            return None;
        }
    };
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let endpoint = match GatewayEndpoint::new(&root) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway URL unusable");
            return None;
        }
    };
    Some(GatewayClient::new(endpoint, key))
}

/// Lists the launchable agent names: the `.md` file stems under `dir`
/// plus the built-in `chat`, sorted. A missing or unreadable directory
/// offers exactly the built-in, and a directory `chat.md` lists once -
/// it shadows the embedded source instead of duplicating the name.
fn discover_agents(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|extension| extension == "md")
        })
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect();
    if !names.iter().any(|name| name == BUILTIN_CHAT_NAME) {
        names.push(BUILTIN_CHAT_NAME.to_owned());
    }
    names.sort();
    names
}

/// Reads the agent's program source: the directory file when it exists -
/// a directory `chat.md` shadows the built-in - else the embedded
/// built-in for the `chat` name alone. Launch resolved `name` through
/// discovery already, so a missing file for any other name is a real
/// filesystem race, surfaced as the error it is; so is an existing
/// `chat.md` that cannot be read, because silently serving the built-in
/// would mask the operator's own file.
fn agent_source(dir: &Path, name: &str) -> io::Result<AgentSource> {
    match std::fs::read_to_string(dir.join(format!("{name}.md"))) {
        Ok(source) => Ok(AgentSource::Markdown(source)),
        Err(error) if name == BUILTIN_CHAT_NAME && error.kind() == io::ErrorKind::NotFound => {
            Ok(AgentSource::Markdown(BUILTIN_CHAT_SOURCE.to_owned()))
        }
        Err(error) => Err(error),
    }
}

/// A fresh unguessable session id: 128 bits from the OS-seeded
/// cryptographic RNG, hex-encoded - wide enough that ids never collide
/// across server restarts, so an old session's JSONL is never truncated
/// by a new session's log.
fn fresh_session_id() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>())
}

#[cfg(test)]
mod tests;
