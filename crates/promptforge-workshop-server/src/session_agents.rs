//! Agent sessions: discovery of `.lua` agent programs, the
//! [`AgentSessions`] registry, and each session's run lifecycle.
//!
//! A session owns one running agent: its persisting event log
//! ([`crate::observer::WorkshopObserver`], JSONL under
//! `state_dir/sessions/<session-id>.jsonl`), its
//! [`crate::input::WaitRegistry`] and `user_input` tool, its dedicated
//! delta broadcast (deltas never enter the event log), and the retained
//! [`CancelHandle`] behind turn-cancel. The supervisor task relaunches
//! `run_agent` over the retained event log after a turn-cancel -
//! cancellation is a stop reason, never an error - and ends the session
//! when the program returns or fails.
//!
//! **Registry carve-out.** Sessions survive socket disconnect and sockets
//! attach and detach ([`socket`]), so this module keeps the session
//! registry the crate's socket rule otherwise forbids. The rule governed
//! per-request relay work, where every held resource belonged to one
//! socket; an agent session is longer-lived than any socket on purpose,
//! and the registry is the one place that owns it.
//!
//! Reply ids coalesce deltas: every live delta is stamped with the id of
//! the durable event that will supersede it. The id is the count of
//! settled model rounds - [`SessionObserver`] advances it as the reply or
//! tool-call event lands, before the program resumes, and the socket
//! derives the same count from the event sequence itself, so both sides
//! agree without sharing more than the log.

pub(crate) mod socket;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use promptforge_core_support::cancel::CancelHandle;
use promptforge_core_support::events::{CallMetrics, RuntimeEventKind, ToolCallEvent};
use promptforge_core_support::observe::{Observation, Observer};
use promptforge_model_client::client::{
    GatewayClient as ModelClient, GatewayEndpoint, SecretString, StreamDelta,
};
use promptforge_model_client::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_store::StoreRef;
use promptforge_tools::{Tool, ToolCatalog};
use tokio::sync::broadcast;
use workshop_agent::{AgentConfig, AgentError, AgentLimits, run_agent_with_client};

use crate::backoff::ReconnectBackoff;
use crate::catalog::CatalogBus;
use crate::input::{UserInputTool, WaitRegistry};
use crate::menu::MenuBus;
use crate::observer::WorkshopObserver;
use crate::protocol::{Activity, AgentDeltaKind, InputFrame};
use crate::push::Push;
use crate::workspace::Workspace;

/// Capacity of a session's delta broadcast. Deltas are ephemeral: a
/// receiver that lags loses chunks, and the completed-reply event is the
/// repair path.
const DELTA_CAPACITY: usize = 256;

/// Capacity of a session's input-frame broadcast. A session holds at
/// most a handful of waits; the registry's retained state is the
/// durable-delivery repair path on lag.
const INPUT_CAPACITY: usize = 32;

/// Context window recorded for a catalog entry that does not carry one.
/// The window is catalog metadata (nothing on the completion wire reads
/// it), so a generous default keeps the model usable rather than
/// refusing it.
const FALLBACK_CONTEXT: u32 = 8192;

/// One live delta on a session's dedicated channel, stamped with the
/// reply id of the durable event that will supersede it.
#[derive(Debug, Clone)]
pub(crate) struct AgentDelta {
    /// The superseding reply id ([`SessionObserver`]'s round count when
    /// the chunk streamed).
    pub(crate) reply: u64,
    /// Which side channel the chunk belongs to.
    pub(crate) channel: AgentDeltaKind,
    /// The chunk's text.
    pub(crate) content: String,
}

/// The shared bus handles a session's lifecycle reports flow through,
/// captured once at [`AgentSessions`] construction.
#[derive(Debug, Clone)]
pub(crate) struct SessionHost {
    /// The status/catalog/menu push facade (Thinking, Generating, idle,
    /// failures).
    pub(crate) push: Push,
    /// Reset on completed replies: an agent reply is useful gateway work.
    pub(crate) backoff: ReconnectBackoff,
    /// Serves `selected_model` to the agent's `ui()` snapshot.
    pub(crate) menu: MenuBus,
    /// Serves `workspace_root` to the agent's `ui()` snapshot: the first
    /// granted root, absent when nothing is granted.
    pub(crate) workspace: Workspace,
    /// The retained gateway catalog the session's model catalog is built
    /// from at launch.
    pub(crate) catalog: CatalogBus,
}

/// The registry of running agent sessions.
///
/// Typed and construction-phased: everything a launch needs is captured
/// when [`AppState`](crate::AppState) builds, and the only mutable state
/// is the session map itself. Sessions survive socket disconnect -
/// sockets attach and detach through [`socket`] - which is this module's
/// documented carve-out from the crate's no-session-registry socket
/// rule.
#[derive(Clone)]
pub struct AgentSessions {
    inner: Arc<Inner>,
}

/// The shared registry state behind the cloneable handle.
struct Inner {
    /// Directory whose `.lua` files are the launchable agents.
    agents_dir: PathBuf,
    /// Where session event JSONLs persist (`state_dir/sessions`).
    sessions_dir: PathBuf,
    /// The model client agents complete through, built from the workshop
    /// gateway settings; `None` when those settings cannot make a client
    /// (an empty API key), which refuses launches rather than failing at
    /// startup - the rest of the workshop still serves.
    client: Option<ModelClient>,
    /// The shared bus handles session lifecycles report through.
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
    /// the shared bus handles. Nothing touches the filesystem here:
    /// discovery reads the agents directory per request, and the
    /// sessions directory is created at first launch.
    pub(crate) fn new(
        agents_dir: PathBuf,
        sessions_dir: PathBuf,
        client: Option<ModelClient>,
        host: SessionHost,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                agents_dir,
                sessions_dir,
                client,
                host,
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The launchable agent names: the `.lua` file stems under the
    /// configured agents directory, sorted. A missing or unreadable
    /// directory is an empty list - a state, not an error.
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
        // bare stem of a real `.lua` file in the configured directory.
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
        let Some(client) = self.inner.client.clone() else {
            return Err(LaunchRefusal::GatewayUnusable);
        };
        let source = std::fs::read_to_string(self.inner.agents_dir.join(format!("{name}.lua")))
            .map_err(|source| LaunchRefusal::SessionState { source })?;
        std::fs::create_dir_all(&self.inner.sessions_dir)
            .map_err(|source| LaunchRefusal::SessionState { source })?;
        let id = fresh_session_id();
        let log_path = self.inner.sessions_dir.join(format!("{id}.jsonl"));
        let observer = Arc::new(
            WorkshopObserver::new(Some(&log_path))
                .map_err(|source| LaunchRefusal::SessionState { source })?,
        );
        let waits = Arc::new(WaitRegistry::new());
        let (input_frames, _) = broadcast::channel(INPUT_CAPACITY);
        let (deltas, _) = broadcast::channel(DELTA_CAPACITY);
        let session = Arc::new(AgentSession {
            id: id.clone(),
            agent: name.to_owned(),
            source,
            log: Arc::clone(&observer),
            rounds: Arc::new(AtomicU64::new(0)),
            waits,
            input_frames,
            deltas,
            cancel: Mutex::new(CancelHandle::new()),
            closing: AtomicBool::new(false),
        });
        self.lock().insert(id, Arc::clone(&session));
        spawn_supervisor(
            Arc::clone(&session),
            self.clone(),
            self.inner.host.clone(),
            client,
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

/// One running agent session: the state that outlives any socket.
pub(crate) struct AgentSession {
    /// The session's unguessable id, also its event JSONL's file stem.
    pub(crate) id: String,
    /// The agent's name (its `.lua` file stem), every observer call's
    /// `section` label.
    pub(crate) agent: String,
    /// The program source, retained so turn-cancel can relaunch it.
    source: String,
    /// The persisting event log: `Observer` write side, `EventLog` read
    /// side, broadcast fan-out for socket wakeups.
    pub(crate) log: Arc<WorkshopObserver>,
    /// Settled model rounds - the reply id deltas are stamped with.
    rounds: Arc<AtomicU64>,
    /// The session's unresolved user-input waits.
    pub(crate) waits: Arc<WaitRegistry>,
    /// Where the `user_input` tool announces waits; sockets subscribe.
    pub(crate) input_frames: broadcast::Sender<InputFrame>,
    /// The dedicated live-delta channel; deltas never enter the event
    /// log.
    deltas: broadcast::Sender<AgentDelta>,
    /// The retained cancel handle of the current run, swapped fresh at
    /// every (re)launch.
    cancel: Mutex<CancelHandle>,
    /// Set by [`close`](Self::close): the supervisor ends instead of
    /// relaunching.
    closing: AtomicBool,
}

impl fmt::Debug for AgentSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSession")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    /// Subscribes to the session's live deltas from this call on.
    pub(crate) fn subscribe_deltas(&self) -> broadcast::Receiver<AgentDelta> {
        self.deltas.subscribe()
    }

    /// Fires the current run's retained cancel handle: the turn dies as
    /// a stop reason (pending waits emit `input_cancelled`, no error
    /// frame), and the supervisor relaunches the program over the
    /// retained event log with a fresh handle.
    pub(crate) fn cancel_turn(&self) {
        self.cancel_guard().cancel();
    }

    /// Ends the session: the run is cancelled and the supervisor stops
    /// relaunching.
    fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.cancel_turn();
    }

    /// Installs and retains the next run's fresh cancel handle.
    fn arm_cancel(&self) -> CancelHandle {
        let fresh = CancelHandle::new();
        *self.cancel_guard() = fresh.clone();
        // A close that raced the swap still wins: cancel the fresh handle
        // at once so the new run cannot outlive the decision to end.
        if self.closing.load(Ordering::SeqCst) {
            fresh.cancel();
        }
        fresh
    }

    /// The cancel-slot guard; poison recovered per the zone-two policy.
    fn cancel_guard(&self) -> MutexGuard<'_, CancelHandle> {
        self.cancel.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The per-session [`Observer`] wrapper `run_agent` reports through: it
/// forwards every report to the persisting log and owns the side effects
/// the session wires to content events - the reply-id round count
/// (advanced as a reply or tool-call batch lands, before the program
/// resumes, so no later delta can carry a settled id), the backoff reset,
/// and the idle status push on completed replies.
struct SessionObserver {
    /// The persisting log every report forwards to.
    log: Arc<WorkshopObserver>,
    /// Settled model rounds, shared with the delta stamp.
    rounds: Arc<AtomicU64>,
    /// Where idle lands when a reply completes.
    push: Push,
    /// Reset on completed replies: the gateway proved it answers.
    backoff: ReconnectBackoff,
}

impl Observer for SessionObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.log.observe(execution, section, event);
    }

    fn on_assistant_reply(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
    ) {
        self.log.on_assistant_reply(
            execution,
            section,
            chain_id,
            depth,
            turn,
            text,
            finish_reason,
            model,
            metrics,
        );
        self.rounds.fetch_add(1, Ordering::SeqCst);
        self.backoff.record_useful_work();
        self.push.push_idle();
    }

    fn on_assistant_tool_calls(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.log
            .on_assistant_tool_calls(execution, section, chain_id, depth, turn, model, calls);
        // A tool-call batch settles its round's deltas without ending the
        // turn: the count advances, the status stays busy.
        self.rounds.fetch_add(1, Ordering::SeqCst);
    }

    fn on_tool_result(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.log.on_tool_result(
            execution,
            section,
            chain_id,
            depth,
            turn,
            tool_call_id,
            alias,
            content,
            trusted,
        );
    }

    fn on_thinking(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.log
            .on_thinking(execution, section, chain_id, depth, turn, model, text);
    }

    fn on_user_input(&self, execution: &str, section: &str, text: &str) {
        self.log.on_user_input(execution, section, text);
    }
}

/// Spawns the session's supervisor: run the agent, relaunch after a
/// turn-cancel over the retained event log with a fresh handle, end the
/// session when the program returns, fails, or the session closes.
fn spawn_supervisor(
    session: Arc<AgentSession>,
    registry: AgentSessions,
    host: SessionHost,
    client: ModelClient,
) {
    tokio::spawn(async move {
        // Per-session pieces that survive relaunches: the tool catalog
        // (`user_input` plus the configured tools - none are configured
        // yet), the model catalog snapshot, the run-scoped store, and
        // the observer wrapper. The event log alone is the state of
        // record; the store is scratch that persisting across relaunches
        // cannot corrupt.
        let tool: Arc<dyn Tool> = Arc::new(UserInputTool::new(
            Arc::clone(&session.waits),
            session.input_frames.clone(),
        ));
        let tools = match ToolCatalog::new(&[tool]) {
            Ok(tools) => tools,
            Err(error) => {
                // Unreachable in practice: the catalog holds one tool
                // with a fixed legal wire name. Refusing the session
                // beats serving an agent that cannot ask for input.
                tracing::error!(%error, session = %session.id, "agent tool catalog refused");
                registry.forget(&session.id);
                return;
            }
        };
        let models = build_model_catalog(host.catalog.latest().map(|push| push.models));
        let store = StoreRef::memory();
        let observer: Arc<dyn Observer> = Arc::new(SessionObserver {
            log: Arc::clone(&session.log),
            rounds: Arc::clone(&session.rounds),
            push: host.push.clone(),
            backoff: host.backoff.clone(),
        });
        let on_delta = delta_stamp(&session, &host.push);
        let ui = ui_provider(&host.menu, &host.workspace);
        loop {
            let config = AgentConfig {
                name: session.agent.clone(),
                execution: session.id.clone(),
                observer: Arc::clone(&observer),
                cancel: session.arm_cancel(),
                event_log: Some(Arc::clone(&session.log) as _),
                on_delta: Some(Arc::clone(&on_delta)),
                ui: Some(Arc::clone(&ui)),
                limits: AgentLimits::default(),
            };
            // Always the workshop's own client: a launch without one was
            // refused, so the environment fallback can never fire here.
            let result = run_agent_with_client(
                &session.source,
                &tools,
                &models,
                &store,
                config,
                Some(client.clone()),
            )
            .await;
            match result {
                // Cancellation is a stop reason, not an error: a
                // turn-cancel relaunches the program over the retained
                // event log; a closing session ends quietly.
                Err(AgentError::Interrupted) => {
                    if session.closing.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Ok(()) => break,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        session = %session.id,
                        agent = %session.agent,
                        "agent run failed"
                    );
                    host.push
                        .push_failure("Agent failed", error.to_string(), Activity::General);
                    break;
                }
            }
        }
        registry.forget(&session.id);
    });
}

/// Builds the delta stamp: the `on_delta` closure feeding the session's
/// dedicated broadcast, each chunk stamped with the current round count -
/// the id of the durable event that will supersede it - plus the
/// activity pulse that lights the status LED (Generating for answer
/// content, Thinking for the reasoning side channel).
fn delta_stamp(session: &Arc<AgentSession>, push: &Push) -> Arc<dyn Fn(StreamDelta) + Send + Sync> {
    let deltas = session.deltas.clone();
    let rounds = Arc::clone(&session.rounds);
    let push = push.clone();
    Arc::new(move |delta| {
        let (channel, content, activity) = match delta {
            StreamDelta::Text(text) => (AgentDeltaKind::Text, text, Activity::Generating),
            StreamDelta::Reasoning(text) => (AgentDeltaKind::Reasoning, text, Activity::Thinking),
            // The enum is non-exhaustive across the crate seam; a future
            // side channel has no frame kind yet and stays live-only.
            _ => return,
        };
        push.push_activity("Streaming response...", "an agent response chunk", activity);
        // No receiver means no socket is attached; deltas are ephemeral
        // and the completed-reply event is the repair, so the drop is
        // the design.
        let _ = deltas.send(AgentDelta {
            reply: rounds.load(Ordering::SeqCst),
            channel,
            content,
        });
    })
}

/// Builds the `ui()` snapshot provider: `selected_model` from the menu's
/// retained workbench state and `workspace_root` as the first granted
/// workspace root, each `null` when absent.
fn ui_provider(
    menu: &MenuBus,
    workspace: &Workspace,
) -> Arc<dyn Fn() -> serde_json::Value + Send + Sync> {
    let menu = menu.clone();
    let workspace = workspace.clone();
    Arc::new(move || {
        let selected = menu.latest().and_then(|snapshot| snapshot.selected_model);
        let root = workspace
            .granted_roots()
            .first()
            .map(|root| root.display().to_string());
        serde_json::json!({ "selected_model": selected, "workspace_root": root })
    })
}

/// The reply-id derivation the socket applies while draining the event
/// log: the model-round content kinds carry the current round count as
/// their stamp, and a reply or tool-call batch advances it - the same
/// rule [`SessionObserver`] applies live, so delta stamps and event
/// stamps agree.
pub(crate) fn reply_stamp(kind: RuntimeEventKind, rounds_seen: &mut u64) -> Option<u64> {
    match kind {
        RuntimeEventKind::Thinking => Some(*rounds_seen),
        RuntimeEventKind::AssistantReply | RuntimeEventKind::AssistantToolCalls => {
            let round = *rounds_seen;
            *rounds_seen += 1;
            Some(round)
        }
        _ => None,
    }
}

/// Lists the launchable agent names: the `.lua` file stems under `dir`,
/// sorted. A missing or unreadable directory is an empty list.
fn discover_agents(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|extension| extension == "lua")
        })
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
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

/// Builds the model-client the agent completes through from the workshop
/// gateway settings: the workshop base URL plus the `/v1` API root.
/// `None` - logged here, and refused per launch as
/// [`LaunchRefusal::GatewayUnusable`] - when the key is empty (the model
/// client refuses blank credentials) or the URL does not parse.
pub(crate) fn model_client(base_url: &str, api_key: &str) -> Option<ModelClient> {
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
    Some(ModelClient::new(endpoint, key))
}

/// Builds the session's model catalog from the retained gateway catalog:
/// one descriptor per chat entry (an absent `kind` is a plain OpenAI
/// catalog and counts as chat), carrying the entry's description,
/// context window, and thinking mode where present. Entries that cannot
/// make a descriptor are skipped with a warning - a launch must not fail
/// because one catalog row is malformed.
fn build_model_catalog(models: Option<Vec<serde_json::Value>>) -> ModelCatalog {
    let Some(models) = models else {
        return ModelCatalog::empty();
    };
    let mut descriptors: Vec<ModelDescriptor> = Vec::new();
    for entry in &models {
        let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
            tracing::warn!("catalog entry without an id skipped for the agent model catalog");
            continue;
        };
        if entry
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind != "chat")
        {
            continue;
        }
        let model_id = match ModelId::gateway(id) {
            Ok(model_id) => model_id,
            Err(error) => {
                tracing::warn!(%error, id, "catalog entry skipped for the agent model catalog");
                continue;
            }
        };
        if descriptors
            .iter()
            .any(|descriptor| descriptor.id() == &model_id)
        {
            tracing::warn!(
                id,
                "duplicate catalog id skipped for the agent model catalog"
            );
            continue;
        }
        let description = entry
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let context = entry
            .get("context")
            .and_then(serde_json::Value::as_u64)
            .and_then(|context| u32::try_from(context).ok())
            .and_then(NonZeroU32::new)
            .unwrap_or_else(|| NonZeroU32::new(FALLBACK_CONTEXT).unwrap_or(NonZeroU32::MIN));
        let thinking = entry
            .get("thinking")
            .and_then(|value| serde_json::from_value::<ThinkingMode>(value.clone()).ok())
            .unwrap_or(ThinkingMode::Never);
        descriptors.push(ModelDescriptor::new(
            model_id,
            description,
            context,
            thinking,
        ));
    }
    // Duplicates were filtered above, so construction cannot refuse; an
    // empty catalog is the honest degenerate outcome.
    ModelCatalog::new(descriptors).unwrap_or_else(|error| {
        tracing::warn!(%error, "agent model catalog degraded to empty");
        ModelCatalog::empty()
    })
}

#[cfg(test)]
mod tests {
    use promptforge_core_support::events::RuntimeEventKind;

    use super::*;

    #[test]
    fn discovery_lists_sorted_lua_stems_and_tolerates_a_missing_dir() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("zeta.lua"), "return 1").expect("seed zeta");
        std::fs::write(dir.path().join("alpha.lua"), "return 1").expect("seed alpha");
        std::fs::write(dir.path().join("notes.txt"), "not an agent").expect("seed noise");
        std::fs::create_dir(dir.path().join("nested.lua")).expect("seed a decoy directory");
        assert_eq!(
            discover_agents(dir.path()),
            vec!["alpha".to_owned(), "zeta".to_owned()],
            "discovery lists .lua file stems sorted and skips everything else"
        );
        assert_eq!(
            discover_agents(&dir.path().join("missing")),
            Vec::<String>::new(),
            "a missing agents directory offers no agents rather than failing"
        );
    }

    #[test]
    fn the_model_catalog_keeps_chat_entries_and_skips_the_rest() {
        let catalog = build_model_catalog(Some(vec![
            serde_json::json!({
                "id": "chat-model", "kind": "chat", "description": "a chat model",
                "context": 4096, "thinking": "switchable",
            }),
            serde_json::json!({ "id": "plain-openai-model" }),
            serde_json::json!({ "id": "embed-model", "kind": "embedding" }),
            serde_json::json!({ "object": "model" }),
            serde_json::json!({ "id": "chat-model" }),
        ]));
        let names: Vec<&str> = catalog
            .models()
            .iter()
            .map(|descriptor| descriptor.id().name())
            .collect();
        assert_eq!(
            names,
            vec!["chat-model", "plain-openai-model"],
            "chat and kind-less entries stay; embeddings, id-less rows, and duplicates drop"
        );
        let chat = &catalog.models()[0];
        assert_eq!(chat.context().get(), 4096);
        assert_eq!(chat.thinking(), ThinkingMode::Switchable);
        let bare = &catalog.models()[1];
        assert_eq!(
            bare.context().get(),
            FALLBACK_CONTEXT,
            "an entry without a context window records the fallback"
        );
        assert!(
            build_model_catalog(None).is_empty(),
            "no retained catalog means an empty agent catalog"
        );
    }

    #[test]
    fn reply_stamps_follow_the_settle_rule() {
        let mut rounds = 0;
        assert_eq!(
            reply_stamp(RuntimeEventKind::UserInput, &mut rounds),
            None,
            "input events settle nothing"
        );
        assert_eq!(
            reply_stamp(RuntimeEventKind::Thinking, &mut rounds),
            Some(0),
            "thinking carries the open round without settling it"
        );
        assert_eq!(
            reply_stamp(RuntimeEventKind::AssistantReply, &mut rounds),
            Some(0)
        );
        assert_eq!(
            reply_stamp(RuntimeEventKind::AssistantToolCalls, &mut rounds),
            Some(1),
            "a tool-call batch settles its round exactly as a reply does"
        );
        assert_eq!(reply_stamp(RuntimeEventKind::ToolResult, &mut rounds), None);
        assert_eq!(
            reply_stamp(RuntimeEventKind::Thinking, &mut rounds),
            Some(2),
            "the next round opens where the last one settled"
        );
    }

    #[test]
    fn the_ui_snapshot_serves_the_selection_and_first_granted_root() {
        let catalog = CatalogBus::default();
        let menu = MenuBus::new(catalog.clone(), None);
        let workspace = Workspace::new();
        let ui = ui_provider(&menu, &workspace);
        assert_eq!(
            ui(),
            serde_json::json!({ "selected_model": null, "workspace_root": null }),
            "absent producers serve null, never a missing key"
        );

        catalog.publish(vec![serde_json::json!({ "id": "test-model" })]);
        menu.set_selected("test-model")
            .expect("the id is in the catalog");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let granted = workspace.grant(dir.path()).expect("the tempdir grants");
        let snapshot = ui();
        assert_eq!(snapshot["selected_model"], "test-model");
        assert_eq!(
            snapshot["workspace_root"],
            serde_json::json!(granted.display().to_string()),
            "workspace_root is the first granted root"
        );
    }

    #[test]
    fn a_launch_without_a_usable_client_is_refused() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("echo.lua"), "return 1").expect("seed echo");
        let catalog = CatalogBus::default();
        let menu = MenuBus::new(catalog.clone(), None);
        let sessions = AgentSessions::new(
            dir.path().to_path_buf(),
            dir.path().join("sessions"),
            None,
            SessionHost {
                push: Push::new(
                    crate::status::StatusBus::new(),
                    catalog.clone(),
                    menu.clone(),
                ),
                backoff: ReconnectBackoff::new(),
                menu,
                workspace: Workspace::new(),
                catalog,
            },
        );
        // A plain #[test] doubles as ordering proof: the refusal returns
        // before anything is spawned, or this panics outside a runtime.
        let refusal = sessions
            .launch("echo")
            .expect_err("a discovered agent must still refuse without a model client");
        assert!(
            matches!(refusal, LaunchRefusal::GatewayUnusable),
            "the refusal names the gateway configuration, not the agent: {refusal}"
        );
        assert!(
            sessions.lock().is_empty(),
            "a refused launch registers no session"
        );
    }

    #[test]
    fn the_model_client_requires_a_usable_key_and_url() {
        assert!(
            model_client("http://127.0.0.1:8081", "k").is_some(),
            "a keyed gateway builds the agent model client"
        );
        assert!(
            model_client("http://127.0.0.1:8081", "").is_none(),
            "an empty key cannot authenticate: agents report it at launch"
        );
        assert!(model_client("not a url", "k").is_none());
    }
}
