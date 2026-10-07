//! The agent-sessions subsystem of the server: the `/agents/ws`
//! agent-session socket ([`socket`]) and its frames ([`wire`]), the
//! `/v1/models` catalog relay ([`relay`]), their shared route state
//! ([`state`]), and [`AgentSessions`], the server's launcher of agent
//! conversations. The `/ws` workshop socket sits outside this subsystem;
//! the two share only [`crate::websocket`].
//!
//! Each launch opens a conversation in `workshop-agents` and builds that
//! conversation's one run its own [`Harness`]: over the conversation's
//! recorder tee on a handle of the run log's [`TursoRecorder`] that names
//! the launched agent, a per-run broker over
//! the server's inference broker ([`broker`]), the tokio timer, a clone of
//! the server's Plugin registry, and a clone of the server's services
//! with the conversation's input broker. The Harness reaches the gateway
//! only through that broker, which follows the live gateway binding and
//! sends each round to the dropdown's current pick. The
//! run's request carries the Host snapshot ([`bindings`]): the menu's
//! pick and the workspace's granted roots, read once the menu's catalog
//! holds a chat-capable model. Status-bar reporting stays in the server
//! (`status`): a per-conversation reporter derives it from the
//! conversation's events, deltas, and failure reports.
//!
//! **Registry carve-out.** Conversations survive socket disconnect and
//! sockets attach and detach (`socket`), so the launcher keeps the
//! conversation table the server's socket rule otherwise forbids. The rule
//! governed per-request relay work, where every held resource belonged to
//! one socket; a conversation is longer-lived than any socket on purpose.

mod bindings;
mod broker;
mod gateway;
mod relay;
mod search;
mod socket;
mod socket_frames;
mod state;
mod status;
mod wire;

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use harness::plugin::{HostServices, PluginRegistry, UserInput};
use harness::record::RunId;
use harness::vfs::VfsRef;
use harness::{Harness, RunRequest, USER_INPUT_ASK_TOOL};
use harness_web::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
use promptforge::tools::ToolId;
use workshop_agents::{
    Conversation, ConversationId, Conversations, LaunchError, TokioTimer, discover_agents,
    load_agent,
};
use workshop_registry::Registry;
use workshop_run_log::TursoRecorder;
use workshop_support::{Config, ReconnectBackoff};

use bindings::host_snapshot;
use broker::{RunBroker, WorkshopBroker};
use gateway::usable_gateway;
use search::GatewaySearchProvider;
pub(crate) use state::{SessionsState, register};

/// The directory under the server's state directory the run log sits in:
/// the `runs.db` every conversation's run is recorded in.
const HARNESS_STATE_DIR: &str = "harness";

/// The Plugins agents may declare: `user-input`, so they
/// can ask the operator, and `web`.
fn plugins() -> PluginRegistry {
    let mut plugins = PluginRegistry::new();
    // Two unrelated ids into an empty registry, so neither registration
    // can be refused.
    let _ = plugins.register(Arc::new(UserInput::new()));
    let _ = plugins.register(Arc::new(Web::new()));
    plugins
}

/// The services `web` reads: the search provider over the
/// gateway `registry` holds, and the runtime the server runs on. Built
/// outside a runtime, as a synchronous test does, the runtime is left
/// out, and a run that requires web is refused. Each run's clone adds its
/// conversation's input broker, so these leave it out.
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

/// The server's launcher of agent conversations: discovery, launch,
/// lookup, and close, plus the server-side work a launch wires up - the
/// run's Harness and the status reporter.
///
/// Typed and construction-phased: the registry, the server's backoff, the
/// run log, and every run's Plugins and services are captured when
/// the composition root builds it, and the other subsystems are read
/// through the registry at the point of use, so this handle never holds
/// another subsystem's handle.
#[derive(Clone)]
pub struct AgentSessions {
    inner: Arc<Inner>,
}

/// The shared state behind the cloneable handle.
struct Inner {
    /// The subsystem registry: the gateway and menu handles, the
    /// workspace roots, and the push facade are read through it.
    registry: Registry,
    /// Reset on completed replies: an agent reply is useful gateway work.
    backoff: ReconnectBackoff,
    /// The folder the launchable agents are discovered in.
    agents_path: PathBuf,
    /// The run log every run is recorded in, through a handle naming its
    /// agent behind each run's tee.
    recorder: Arc<TursoRecorder>,
    /// The inference broker behind each run's broker.
    broker: WorkshopBroker,
    /// The Plugins every run resolves its declarations against.
    plugins: PluginRegistry,
    /// The ask tool's id, which a script's ask result is recognized by;
    /// `None` only if the id fails to parse.
    ask: Option<ToolId>,
    /// The services every run's clone starts from.
    services: HostServices,
    /// The running conversations.
    conversations: Conversations,
}

impl fmt::Debug for AgentSessions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSessions")
            .field("conversations", &self.inner.conversations)
            .finish_non_exhaustive()
    }
}

impl AgentSessions {
    /// Builds the launcher for `config` over the subsystem registry and
    /// the server's reconnect backoff. Nothing is spawned and nothing
    /// touches the filesystem here: the run log opens under the state
    /// directory when the first run starts.
    #[must_use]
    pub fn new(config: &Config, registry: Registry, backoff: ReconnectBackoff) -> Self {
        Self {
            inner: Arc::new(Inner {
                agents_path: config.agents.path.clone(),
                recorder: Arc::new(TursoRecorder::new(
                    config.server.state_dir.join(HARNESS_STATE_DIR),
                )),
                broker: WorkshopBroker::new(registry.clone()),
                plugins: plugins(),
                ask: ToolId::parse(USER_INPUT_ASK_TOOL).ok(),
                services: services(&registry),
                conversations: Conversations::new(),
                registry,
                backoff,
            }),
        }
    }

    /// The launchable agent names: the `.md` file stems under the
    /// configured agents directory plus the built-in `chat`, sorted.
    #[must_use]
    pub fn discover(&self) -> Vec<String> {
        discover_agents(&self.inner.agents_path)
    }

    /// Launches a conversation running the discovered agent `name` and
    /// returns it. Its run goes until its program returns, fails, or
    /// [`close`](Self::close) ends it; a stop drops only the round in
    /// flight.
    ///
    /// A launch with no usable gateway is refused before the agent is
    /// read, since its every model round would fail.
    ///
    /// # Errors
    /// Returns [`LaunchRefusal::GatewayUnusable`] when no gateway is
    /// registered or its URL or key cannot build, and the conversation's
    /// own [`LaunchError`] otherwise: an unknown agent or an unreadable
    /// agent source.
    async fn launch(&self, name: &str) -> Result<Conversation, LaunchRefusal> {
        if usable_gateway(&self.inner.registry).is_none() {
            return Err(LaunchRefusal::GatewayUnusable);
        }
        // The folder walk and the source read are filesystem work, so
        // they run on the blocking pool; a worker that cannot report is
        // the same unreadable state as an unreadable file.
        let agents_path = self.inner.agents_path.clone();
        let agent = name.to_owned();
        let source = tokio::task::spawn_blocking(move || load_agent(&agents_path, &agent))
            .await
            .unwrap_or_else(|join| {
                Err(LaunchError::Unreadable {
                    source: io::Error::other(join),
                })
            })?;
        let conversation = self.inner.conversations.open(name);
        status::spawn_reporter(
            &conversation,
            self.inner.registry.push(),
            self.inner.backoff.clone(),
        );
        tokio::spawn(self.clone().drive(conversation.clone(), source));
        Ok(conversation)
    }

    /// Runs `conversation`'s one run over `source`. The request's Host
    /// snapshot is read once the menu's catalog holds a chat-capable
    /// model, so a pick made while the catalog was empty is the one the
    /// run binds; a close during that wait ends the run before it begins.
    async fn drive(self, conversation: Conversation, source: String) {
        tokio::select! {
            () = conversation.closing() => {}
            () = self.inner.broker.chat_model_ready() => {}
        }
        let request = RunRequest {
            name: conversation.id().to_string(),
            source,
            args: String::new(),
            input_text: None,
            vfs: VfsRef::default(),
            host: host_snapshot(&self.inner.registry),
        };
        conversation
            .run(self.harness_for(&conversation), request)
            .await;
    }

    /// The Harness for `conversation`'s run.
    fn harness_for(&self, conversation: &Conversation) -> Harness {
        Harness::new(
            conversation.recorder(Arc::new(
                self.inner.recorder.for_agent(conversation.agent()),
            )),
            Arc::new(RunBroker::new(
                self.inner.broker.clone(),
                conversation.clone(),
            )),
            Arc::new(TokioTimer),
            self.inner.plugins.clone(),
            conversation.services(&self.inner.services),
        )
    }

    /// The ask tool's id, which a socket frames a script's ask result by.
    fn ask(&self) -> Option<ToolId> {
        self.inner.ask.clone()
    }

    /// The running conversation with this id, when one exists: how a
    /// socket reattaches after a disconnect.
    fn get(&self, id: &str) -> Option<Conversation> {
        self.inner.conversations.get(&ConversationId::new(id))
    }

    /// Ends the conversation with this id: its run is cancelled, pending
    /// waits die as `input_cancelled`, and the conversation leaves the
    /// table at once. Returns whether a conversation was ended.
    #[must_use]
    pub fn close(&self, id: &str) -> bool {
        self.inner.conversations.close(&ConversationId::new(id))
    }

    /// The unresolved wait tokens of the conversation with this id - the
    /// teardown leak probe: after a close or a finished run, the list
    /// must be empty. `None` when no such conversation is running.
    #[must_use]
    pub fn unresolved_waits(&self, id: &str) -> Option<Vec<String>> {
        Some(self.get(id)?.unresolved_waits())
    }

    /// The one run of the running conversation with this id, as the run
    /// log issued it; `None` when no such conversation is running or its
    /// run has not begun.
    #[must_use]
    pub fn run_id(&self, id: &str) -> Option<RunId> {
        self.get(id)?.run_id()
    }

    /// Delivers a fixture response after running `after_acceptance`
    /// between its acceptance and the waiting ask's resumption.
    #[cfg(feature = "test-fixtures")]
    pub fn deliver_input_after_acceptance_for_test(
        &self,
        id: &str,
        response: workshop_protocol::InputResponse,
        after_acceptance: impl FnOnce(),
    ) -> Option<Result<(), workshop_agents::WaitError>> {
        let conversation = self.get(id)?;
        Some(conversation.send_input(&response.token, response.text, after_acceptance))
    }
}

/// A refused agent launch, relayed to the client as an error frame.
#[derive(Debug, thiserror::Error)]
enum LaunchRefusal {
    /// No gateway is registered, or the registered gateway's URL or key
    /// cannot build, so no agent could complete a model round.
    #[error("agent sessions need a usable gateway binding; check the gateway base URL and key")]
    GatewayUnusable,
    /// The agent was refused.
    #[error(transparent)]
    Refused(#[from] LaunchError),
}

#[cfg(test)]
mod tests;
