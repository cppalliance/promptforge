//! The agent-sessions subsystem of the server: the `/agents/ws`
//! agent-session socket ([`socket`]) and its frames ([`wire`]), the
//! `/v1/models` catalog relay ([`relay`]), their shared route state
//! ([`state`]), and
//! [`AgentSessions`], the server's opener of agent sessions in the
//! Harness. The `/ws` workshop socket sits outside this subsystem; the
//! two share only [`crate::websocket`].
//!
//! Agent sessions run in the Harness. The composition root constructs a
//! [`Harness`] from `harness` and registers it like every other
//! subsystem handle; this module reaches it through the registry and opens
//! every session through it. Everything the Harness knows about the server
//! arrives as data pushed through its public API ([`bindings`]): the
//! gateway endpoint and bearer, the chat-capable catalog, and the Host
//! snapshot (the menu's selection and the workspace's granted roots).
//! Status-bar reporting stays in the server (`status`): a per-session
//! reporter derives it from the session's events, deltas, and error reports.
//!
//! **Registry carve-out.** Sessions survive socket disconnect and sockets
//! attach and detach (`socket`), so the Harness keeps the session table
//! the server's socket rule otherwise forbids. The rule governed
//! per-request relay work, where every held resource belonged to one
//! socket; an agent session is longer-lived than any socket on purpose.

mod bindings;
pub(crate) mod relay;
mod search;
pub(crate) mod socket;
pub(crate) mod socket_frames;
pub(crate) mod state;
mod status;
mod wire;

use std::fmt;
use std::sync::Arc;

use harness::capability::{CapabilityRegistry, HostServices, UserInput};
use harness::{Harness, HarnessConfig, LaunchError, LaunchRequest, Session, SessionId};
use harness_web::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
use workshop_registry::Registry;
use workshop_run_log::TursoRecorder;
use workshop_support::{Config, ReconnectBackoff};

#[cfg(feature = "test-fixtures")]
pub(crate) use bindings::forward as forward_bindings;
use bindings::push_bindings;
use search::GatewaySearchProvider;
pub(crate) use state::{SessionsState, register, register_tasks};

/// The directory under the server's state directory the run log sits in:
/// the `runs.db` every agent session's runs are recorded in.
const HARNESS_STATE_DIR: &str = "harness";

/// The Harness every agent session runs in, built for `config` with the
/// server's current state already pushed through its public API: the
/// gateway endpoint and bearer, the chat catalog, and the Host snapshot,
/// each read through `registry` from the subsystems registered before it.
/// Its capabilities are [`capabilities`] and its services
/// [`services`]. The composition root registers the returned handle and
/// the forwarder task ([`register_tasks`]) that keeps the bindings current
/// from the buses once the server serves. Nothing touches the filesystem
/// here: the run log opens under the state directory when the first run
/// starts.
pub(crate) fn harness_for(config: &Config, registry: &Registry) -> Arc<Harness> {
    let harness = Arc::new(Harness::new(
        HarnessConfig {
            agents_path: config.agents.path.clone(),
        },
        Arc::new(TursoRecorder::new(
            config.server.state_dir.join(HARNESS_STATE_DIR),
        )),
        capabilities(),
        services(registry),
    ));
    push_bindings(registry, &harness);
    harness
}

/// The capabilities agents may declare: `promptforge/user-input`, so they
/// can ask the operator, and `promptforge/web`.
fn capabilities() -> CapabilityRegistry {
    let mut capabilities = CapabilityRegistry::new();
    // Two unrelated ids into an empty registry, so neither registration
    // can be refused.
    let _ = capabilities.register(Arc::new(UserInput::new()));
    let _ = capabilities.register(Arc::new(Web::new()));
    capabilities
}

/// The services `promptforge/web` reads: the search provider over the
/// gateway `registry` holds, and the runtime the server runs on. Built
/// outside a runtime, as a synchronous test does, the runtime is left
/// out, and a run that requires web is refused.
fn services(registry: &Registry) -> HostServices {
    let mut services = HostServices::new();
    // Two valid, distinct literals into an empty map, so neither call
    // can be refused.
    let _ = services.provide(
        &SEARCH_PROVIDER,
        Arc::new(GatewaySearchProvider::new(registry.clone())),
    );
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let _ = services.provide(&TOKIO_RUNTIME, Arc::new(runtime));
    }
    services
}

/// The server's opener of agent sessions: discovery, launch, and lookup
/// through the registered [`Harness`], plus the server-side work a launch
/// wires up - the status reporter.
///
/// Typed and construction-phased: the registry and the server's backoff
/// are captured when the composition root builds it, and the Harness is
/// read through the registry at the point of use, so this handle never
/// holds another subsystem's handle.
#[derive(Clone)]
pub struct AgentSessions {
    inner: Arc<Inner>,
}

/// The shared state behind the cloneable handle.
struct Inner {
    /// The subsystem registry: the Harness, the gateway and menu handles,
    /// the workspace roots, and the push facade are read through it.
    registry: Registry,
    /// Reset on completed replies: an agent reply is useful gateway work.
    backoff: ReconnectBackoff,
}

impl fmt::Debug for AgentSessions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSessions")
            .finish_non_exhaustive()
    }
}

impl AgentSessions {
    /// Builds the opener over the subsystem registry and the server's
    /// reconnect backoff. Nothing is spawned here; the composition root
    /// runs outside the runtime.
    #[must_use]
    pub fn new(registry: Registry, backoff: ReconnectBackoff) -> Self {
        Self {
            inner: Arc::new(Inner { registry, backoff }),
        }
    }

    /// The registered Harness, or `None` while the composition root has
    /// not registered one - every operation then degrades to its empty
    /// answer.
    fn harness(&self) -> Option<Arc<Harness>> {
        self.inner.registry.state::<Harness>()
    }

    /// The launchable agent names: the `.md` file stems under the
    /// configured agents directory plus the built-in `chat`, sorted. An
    /// unregistered Harness discovers nothing.
    #[must_use]
    pub fn discover(&self) -> Vec<String> {
        self.harness()
            .map_or_else(Vec::new, |harness| harness.discover())
    }

    /// Pushes the server's current gateway, catalog, and Host state into
    /// the Harness, so the next run the Harness prepares reads them.
    pub(crate) fn sync_bindings(&self) {
        if let Some(harness) = self.harness() {
            push_bindings(&self.inner.registry, &harness);
        }
    }

    /// Launches a session running the discovered agent `name` and returns
    /// its handle. The session runs until its program returns, fails, or
    /// [`close`](Self::close) ends it; turn-cancel relaunches the program
    /// over the retained transcript without ending the session.
    ///
    /// The server's bindings are pushed first, so the launch reads the
    /// current selection and roots even when the forwarder task has not
    /// caught up with the latest replacement.
    ///
    /// # Errors
    /// Returns [`LaunchRefusal::Unavailable`] when no Harness is
    /// registered, and the Harness's own [`LaunchError`] otherwise: an
    /// unknown agent, an unusable gateway, or unreadable agent source.
    pub(crate) async fn launch(&self, name: &str) -> Result<Session, LaunchRefusal> {
        let harness = self.harness().ok_or(LaunchRefusal::Unavailable)?;
        push_bindings(&self.inner.registry, &harness);
        let session = harness
            .launch(LaunchRequest {
                agent: name.to_owned(),
                args: String::new(),
                input_text: None,
            })
            .await?;
        status::spawn_reporter(
            &session,
            self.inner.registry.push(),
            self.inner.backoff.clone(),
        );
        Ok(session)
    }

    /// The running session with this id, when one exists: how a socket
    /// reattaches after a disconnect.
    pub(crate) fn get(&self, id: &str) -> Option<Session> {
        self.harness()?.session(&SessionId::new(id))
    }

    /// Ends the session with this id: its run is cancelled for good (no
    /// relaunch), pending waits die as `input_cancelled`, and the session
    /// leaves the Harness. Returns whether a session was ended.
    #[must_use]
    pub fn close(&self, id: &str) -> bool {
        self.harness()
            .is_some_and(|harness| harness.close(&SessionId::new(id)))
    }

    /// The unresolved wait tokens of the session with this id - the
    /// teardown leak probe: after a close or a finished run, the list
    /// must be empty. `None` when no such session is running.
    #[must_use]
    pub fn unresolved_waits(&self, id: &str) -> Option<Vec<String>> {
        Some(self.get(id)?.unresolved_waits())
    }

    /// Delivers a fixture response after running `after_acceptance`
    /// between its acceptance and the waiting ask's resumption.
    #[cfg(feature = "test-fixtures")]
    pub fn deliver_input_after_acceptance_for_test(
        &self,
        id: &str,
        response: workshop_protocol::InputResponse,
        after_acceptance: impl FnOnce(),
    ) -> Option<Result<(), harness::WaitError>> {
        let session = self.get(id)?;
        Some(session.send_input(&response.token, response.text, after_acceptance))
    }
}

/// A refused agent launch, relayed to the client as an error frame.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchRefusal {
    /// The composition root registered no Harness.
    #[error("agent sessions are unavailable")]
    Unavailable,
    /// The Harness refused the launch.
    #[error(transparent)]
    Refused(#[from] LaunchError),
}

#[cfg(test)]
mod tests;
