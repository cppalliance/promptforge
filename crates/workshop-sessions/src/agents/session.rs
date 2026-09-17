//! One running agent session: the state that outlives any socket, the
//! per-session observer the agent run reports through, and the
//! launch-time providers for deltas and the `ui()` snapshot.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::events::{CallMetrics, RuntimeEventKind, ToolCallEvent};
use promptforge_api_types::observe::{Observation, Observer};
use promptforge_api_types::wire::StreamDelta;
use tokio::sync::broadcast;

use workshop_gateway::WorkshopObserver;
use workshop_menu::MenuBus;
use workshop_protocol::{Activity, AgentDeltaKind, InputFrame, InputResponse};
use workshop_registry::{Push, Registry};

use super::lifecycle::RunLifecycle;
use super::supervisor::transition::RunId;
use crate::input::{WaitError, WaitRegistry};

/// One agent's program source: a Markdown prompt document on the
/// unified runtime. Directory agents and the embedded built-in chat are
/// both Markdown; the standalone Lua agent path is retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentSource {
    /// A Markdown prompt document (the unified runtime).
    Markdown(String),
}

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

/// One running agent session: the state that outlives any socket.
pub(crate) struct AgentSession {
    /// The session's unguessable id, also its event JSONL's file stem.
    pub(crate) id: String,
    /// The agent's name (its `.md` file stem), every observer call's
    /// `section` label.
    pub(crate) agent: String,
    /// The program source and its runtime, retained so turn-cancel can
    /// relaunch it.
    pub(super) source: AgentSource,
    /// The persisting event log: `Observer` write side, `EventLog` read
    /// side, broadcast fan-out for socket wakeups.
    pub(crate) log: Arc<WorkshopObserver>,
    /// Cancellation provenance and the accepted-turn exclusion boundary.
    pub(super) lifecycle: Arc<RunLifecycle>,
    /// Settled model rounds - the reply id deltas are stamped with.
    pub(super) rounds: Arc<AtomicU64>,
    /// The session's unresolved user-input waits.
    pub(crate) waits: Arc<WaitRegistry>,
    /// Where the `user_input` tool announces waits; sockets subscribe.
    pub(crate) input_frames: broadcast::Sender<InputFrame>,
    /// The dedicated live-delta channel; deltas never enter the event
    /// log.
    pub(super) deltas: broadcast::Sender<AgentDelta>,
    /// The session's error reports, forwarded to the SPA as `error`
    /// frames: a failed model round the program survived, or a run that
    /// ended in error. Ephemeral like the deltas - errors never enter
    /// the event log.
    pub(super) errors: broadcast::Sender<String>,
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
    /// Bundles one session's state at launch.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: String,
        agent: &str,
        source: AgentSource,
        log: Arc<WorkshopObserver>,
        lifecycle: Arc<RunLifecycle>,
        waits: Arc<WaitRegistry>,
        input_frames: broadcast::Sender<InputFrame>,
        deltas: broadcast::Sender<AgentDelta>,
        errors: broadcast::Sender<String>,
    ) -> Self {
        Self {
            id,
            agent: agent.to_owned(),
            source,
            log,
            lifecycle,
            rounds: Arc::new(AtomicU64::new(0)),
            waits,
            input_frames,
            deltas,
            errors,
        }
    }

    /// Subscribes to the session's live deltas from this call on.
    pub(crate) fn subscribe_deltas(&self) -> broadcast::Receiver<AgentDelta> {
        self.deltas.subscribe()
    }

    /// Subscribes to the session's error reports from this call on.
    pub(crate) fn subscribe_errors(&self) -> broadcast::Receiver<String> {
        self.errors.subscribe()
    }

    /// Durably accepts one input and resumes its wait after publishing
    /// acceptance ahead of the observation-to-completion boundary.
    ///
    /// Recording is consumer-side: the unified runtime records the
    /// operator's text when the suspended `user_input` resumes, so
    /// recording here too would double the event.
    pub(crate) fn accept_input(
        &self,
        response: InputResponse,
        after_acceptance: impl FnOnce(),
    ) -> Result<(), WaitError> {
        let accepted_run = self.lifecycle.accept_input();
        let result = crate::input::complete_input_response(&self.waits, response, after_acceptance);
        if let (Err(_), Some(run)) = (&result, accepted_run) {
            self.lifecycle.settle_turn(run);
        }
        result
    }

    /// Fires the current run's retained cancel handle: the turn dies as
    /// a stop reason (pending waits emit `input_cancelled`, no error
    /// frame), and the supervisor relaunches the program over the
    /// retained event log with a fresh handle.
    pub(crate) fn cancel_turn(&self) {
        self.lifecycle.operator_cancel();
    }

    /// Ends the session: the run is cancelled and the supervisor stops
    /// relaunching.
    pub(super) fn close(&self) {
        self.lifecycle.close();
    }

    /// Installs and retains the next run's fresh cancel handle.
    pub(super) fn arm_cancel(&self, run: RunId) -> CancelHandle {
        self.lifecycle.arm(run)
    }

    /// Cancels the run selected by a reducer effect.
    pub(super) fn cancel_current_run(&self) {
        self.lifecycle.cancel_current();
    }

    /// Clears the lifecycle identity after a run ends.
    pub(super) fn finish_run(&self, run: RunId) {
        self.lifecycle.finish(run);
    }
}

/// The per-session [`Observer`] wrapper `run_agent` reports through: it
/// forwards every report to the persisting log and owns the side effects
/// the session wires to content events - the reply-id round count
/// (advanced as a reply or tool-call batch lands, before the program
/// resumes, so no later delta can carry a settled id), the backoff reset,
/// and the idle status push on completed replies.
pub(crate) struct SessionObserver {
    /// The persisting log every report forwards to.
    pub(super) log: Arc<WorkshopObserver>,
    /// Settled model rounds, shared with the delta stamp.
    pub(super) rounds: Arc<AtomicU64>,
    /// Where idle lands when a reply completes.
    pub(super) push: Push,
    /// Reset on completed replies: the gateway proved it answers.
    pub(super) backoff: workshop_support::ReconnectBackoff,
    /// Where a failed model round surfaces as a wire error frame.
    pub(super) errors: broadcast::Sender<String>,
    /// Marks an accepted turn settled before catalog retirement proceeds.
    pub(super) lifecycle: Arc<RunLifecycle>,
}

impl Observer for SessionObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        // A failed model round or tool dispatch is operator-visible: the
        // program survives it (the built-in chat pcalls models.loop and
        // returns to waiting), so the run never fails and only the session
        // can tell the SPA. A tool dispatch failure aborts the loop just as
        // a failed round does, so both are terminal for the turn. The
        // observation carries no payload; the frame names the boundary
        // that failed.
        if matches!(
            event,
            Observation::ModelTurnFailed | Observation::ToolCallFailed
        ) {
            self.lifecycle.settle_current_turn();
            let message = format!("{event} in agent `{section}`");
            let _ = self.errors.send(message.clone());
            // The failed turn never reaches on_assistant_reply, so this
            // terminal status is the only frame that releases the
            // turn-dispatch Thinking push; without it the status bar's
            // sustained amber LED never returns to idle.
            self.push
                .push_failure(event.to_string(), message, Activity::General);
        }
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
        self.lifecycle.settle_current_turn();
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

/// Builds the delta stamp: the `on_delta` closure feeding the session's
/// dedicated broadcast, each chunk stamped with the current round count -
/// the id of the durable event that will supersede it - plus the
/// activity pulse that lights the status LED (Generating for answer
/// content, Thinking for the reasoning side channel).
pub(crate) fn delta_stamp(
    session: &Arc<AgentSession>,
    push: &Push,
) -> Arc<dyn Fn(StreamDelta) + Send + Sync> {
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
/// workspace root, each `null` when absent. The roots come through the
/// registry's state collection, so this crate never names the workspace
/// crate the tier graph forbids; an unregistered handle serves `null`.
pub(crate) fn ui_provider(
    menu: &MenuBus,
    registry: &Registry,
) -> Arc<dyn Fn() -> serde_json::Value + Send + Sync> {
    let menu = menu.clone();
    let registry = registry.clone();
    Arc::new(move || {
        let selected = menu.latest().and_then(|snapshot| snapshot.selected_model);
        let root = registry
            .state::<dyn workshop_registry::WorkspaceRoots>()
            .and_then(|roots| roots.granted_roots().first().cloned())
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
