//! One agent session: the handle a client holds, the state that outlives
//! any client connection, and the sink each run of the session reports
//! through.
//!
//! A session owns one running agent. Its transcript is the run log: every
//! event of every run the session has made, in log order, numbered from
//! zero across relaunches; a live subscriber receives each event as it is
//! recorded and a reconnecting client reads [`Session::transcript`] past
//! its last seen index. Deltas are sent on a separate broadcast, stamped
//! with the reply id of the event that will supersede them, and never enter
//! the log. The session's unresolved input waits, its delta and wait
//! channels, and its lifecycle survive a client's disconnect; the
//! supervisor (`session::supervisor`) relaunches the program over the
//! retained transcript after a turn-cancel and ends the session when the
//! program returns or fails.
//!
//! Reply ids coalesce deltas: every live delta is stamped with the id of
//! the durable event that will supersede it. The id is the count of
//! settled model rounds - the core's sink advances it as the reply or
//! tool-call event lands, before the program resumes - and the transcript
//! read derives the same count from the event sequence through the one
//! rule [`reply_stamp`], so live and replayed stamps agree.

pub(crate) mod run;
pub(crate) mod supervisor;

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use harness_log::{LogError, RunId as LogRunId};
use harness_runner::effect_loop::SharedLog;
use promptforge_api_types::event::Event;
use promptforge_api_types::wire::StreamDelta;
use tokio::sync::{broadcast, mpsc, watch};

use crate::discovery::AgentSource;
use crate::input::{WaitError, WaitFrame, WaitRegistry, complete_input_response};
use crate::lifecycle::RunLifecycle;
use crate::protocol::{Delta, DeltaKind, SessionEvent, SessionId};
use crate::transition::{RunId, SessionState};

/// Capacity of a session's event broadcast. The broadcast is the wakeup;
/// a receiver that lags repairs by reading the transcript past its
/// cursor.
pub const EVENT_CAPACITY: usize = 256;

/// Capacity of a session's delta broadcast. Deltas are ephemeral: a
/// receiver that lags loses chunks, and the completed-reply event is the
/// repair path.
pub const DELTA_CAPACITY: usize = 256;

/// Capacity of a session's input-frame broadcast. A session holds at
/// most a handful of waits; the registry's retained state is the
/// durable-delivery repair path on lag.
pub const INPUT_CAPACITY: usize = 32;

/// Capacity of a session's error broadcast. Session errors are rare
/// one-off reports: a failed model round or a run that ended in error
/// surfaces one frame each, and a receiver that lags misses only what
/// the durable transcript already shows as a turn without a reply.
pub const ERROR_CAPACITY: usize = 8;

/// What kind of failure a session is reporting: the machine-readable
/// fact a client classifies on. The first two are turn failures the
/// program survived (the built-in chat pcalls `models.loop` and returns
/// to waiting); the last two end the run. Deliberately not
/// `#[non_exhaustive]`: a client that labels each kind matches on it
/// exhaustively, so a new kind fails that client's build until it is
/// labelled instead of silently falling into a wildcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// A model round failed; the program survived and is waiting again.
    ModelTurnFailed,
    /// A tool dispatch failed; the program survived and is waiting again.
    ToolCallFailed,
    /// The run itself ended in error.
    RunFailed,
    /// A requested close interrupted the run before a genuine terminal;
    /// this is the synthetic terminal the supervisor renders after the
    /// drain.
    Interrupted,
}

/// One operator-facing failure report: the kind is the fact code acts
/// on, the message is display text for the operator and the model. Code
/// never derives meaning from the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFailure {
    /// Which failure this is.
    pub kind: FailureKind,
    /// The sentence a client shows for it.
    pub message: String,
}

/// A live agent session: the handle a client launches, sends input to,
/// cancels, closes, and subscribes to events and deltas through. Cheap
/// to clone; every clone names the same session.
#[derive(Clone)]
pub struct Session {
    core: Arc<SessionCore>,
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("id", &self.core.id)
            .field("agent", &self.core.agent)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn new(core: Arc<SessionCore>) -> Self {
        Self { core }
    }

    /// The session's id.
    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.core.id
    }

    /// The agent the session runs: its name as discovered.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.core.agent
    }

    /// Where the current run stands: [`SessionState::Alive`] while it
    /// steps, [`SessionState::Closing`] once cancel or close was requested
    /// and its outstanding effects are being answered or dropped, and
    /// [`SessionState::Closed`] once the run reported `Done`.
    #[must_use]
    pub fn state(&self) -> SessionState {
        *self.core.state.borrow()
    }

    /// A watch on [`Session::state`]: what a client awaits to see a close
    /// drain to `Closed`.
    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<SessionState> {
        self.core.state.subscribe()
    }

    /// Subscribes to the session's events from this call on. Subscribe
    /// first, then read [`Session::transcript`] past the last seen index,
    /// and skip live events below the cursor: nothing is lost between the
    /// two.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<SessionEvent> {
        self.core.events.subscribe()
    }

    /// Subscribes to the session's live deltas from this call on.
    #[must_use]
    pub fn subscribe_deltas(&self) -> broadcast::Receiver<Delta> {
        self.core.deltas.subscribe()
    }

    /// Subscribes to the session's input-wait frames from this call on.
    /// Call [`Session::resend_waits`] after subscribing to learn of waits
    /// already open.
    #[must_use]
    pub fn subscribe_waits(&self) -> broadcast::Receiver<WaitFrame> {
        self.core.wait_frames.subscribe()
    }

    /// Subscribes to the session's operator-facing failure reports from
    /// this call on. Each includes its [`FailureKind`] - a failed model
    /// round or tool call the program survived, a run that ended in
    /// error, or an interrupt's synthetic terminal - beside its display
    /// message. Ephemeral like the deltas.
    #[must_use]
    pub fn subscribe_errors(&self) -> broadcast::Receiver<SessionFailure> {
        self.core.errors.subscribe()
    }

    /// The unresolved wait tokens in creation order: the teardown leak
    /// probe, empty after a close or a finished run.
    #[must_use]
    pub fn unresolved_waits(&self) -> Vec<String> {
        self.core.waits.unresolved()
    }

    /// Re-announces every unresolved wait on the wait-frame broadcast, in
    /// creation order: the reconnect half of durable delivery.
    pub fn resend_waits(&self) {
        self.core.waits.resend_unresolved(&self.core.wait_frames);
    }

    /// Answers the wait holding `token` with the operator's `text`.
    /// `before_resume` runs after the answer is accepted and before the
    /// suspended `user_input()` call resumes, for a client's own turn
    /// bookkeeping.
    ///
    /// # Errors
    /// Returns [`WaitError::UnknownToken`] when no unresolved wait holds
    /// `token`.
    pub fn send_input(
        &self,
        token: &str,
        text: String,
        before_resume: impl FnOnce(),
    ) -> Result<(), WaitError> {
        let accepted_run = self.core.lifecycle.accept_input();
        let result = complete_input_response(&self.core.waits, token, text, before_resume);
        if let (Err(_), Some(run)) = (&result, accepted_run) {
            self.core.lifecycle.settle_turn(run);
        }
        result
    }

    /// Cancels the current turn: the run dies as a stop reason (pending
    /// waits emit a cancelled frame, no error report), and the supervisor
    /// relaunches the program over the retained transcript.
    pub fn cancel(&self) {
        self.core.interrupted();
        self.core.lifecycle.operator_cancel();
    }

    /// Ends the session: the run is cancelled for good, its outstanding
    /// effects are answered `Dropped`, and once it reports `Done` the
    /// state is `Closed` and the session leaves its harness.
    pub fn close(&self) {
        self.core.interrupted();
        self.core.lifecycle.close();
    }

    /// The runs the session has made, in launch order, as the log knows
    /// them.
    #[must_use]
    pub fn run_ids(&self) -> Vec<LogRunId> {
        self.core.run_ids()
    }

    /// The session's transcript from index `from` on: every event of
    /// every run, in log order, read from the run log. Replaces the
    /// in-memory log for a reconnecting client and a transcript view.
    ///
    /// # Errors
    /// Returns the log's error when a run cannot be read or a stored
    /// payload no longer parses as an event.
    pub async fn transcript(&self, from: u64) -> Result<Vec<SessionEvent>, LogError> {
        let mut index = 0u64;
        let mut rounds_seen = 0u64;
        let mut transcript = Vec::new();
        for run in self.core.run_ids() {
            let records = self.core.log.lock().await.transcript(run).await?;
            for stored in records {
                let event: Event = serde_json::from_value(stored.record.payload.clone())?;
                let reply = reply_stamp(&event, &mut rounds_seen);
                if index >= from {
                    transcript.push(SessionEvent {
                        index,
                        reply,
                        event: stored.record.payload,
                    });
                }
                index += 1;
            }
        }
        Ok(transcript)
    }
}

/// One running agent session: the state that outlives any client
/// connection.
pub(crate) struct SessionCore {
    /// The session's unguessable id, every run's execution identifier.
    pub(crate) id: SessionId,
    /// The agent's name, the run row's `agent`.
    pub(crate) agent: String,
    /// The program source, retained so turn-cancel can relaunch it.
    pub(crate) source: AgentSource,
    /// The path the source is attributed to in parse failures.
    pub(crate) prompt_path: PathBuf,
    /// The run's argument text.
    pub(crate) args: String,
    /// Cancellation provenance and the accepted-turn exclusion boundary.
    pub(crate) lifecycle: Arc<RunLifecycle>,
    /// The session's unresolved user-input waits.
    pub(crate) waits: Arc<WaitRegistry>,
    /// Where the input broker announces waits.
    pub(crate) wait_frames: broadcast::Sender<WaitFrame>,
    /// The live event broadcast; the transcript is the log.
    events: broadcast::Sender<SessionEvent>,
    /// The dedicated live-delta channel; deltas never enter the log.
    deltas: broadcast::Sender<Delta>,
    /// The session's failure reports.
    pub(crate) errors: broadcast::Sender<SessionFailure>,
    /// The sink the chat performers stream raw deltas to; the supervisor
    /// drains its receiver and stamps each delta. Held here so the
    /// channel never closes while the session lives.
    pub(crate) delta_source: mpsc::UnboundedSender<StreamDelta>,
    /// Settled model rounds: the reply id deltas are stamped with.
    rounds: AtomicU64,
    /// The next transcript index a live event takes.
    next_index: AtomicU64,
    /// The session's runs in launch order.
    runs: Mutex<Vec<LogRunId>>,
    /// Where the current run stands.
    state: watch::Sender<SessionState>,
    /// The run log every run is recorded in and the transcript is read
    /// from.
    pub(crate) log: SharedLog,
}

/// What a launch hands the core beyond its channels.
pub(crate) struct SessionSeed {
    pub(crate) id: SessionId,
    pub(crate) agent: String,
    pub(crate) source: AgentSource,
    pub(crate) prompt_path: PathBuf,
    pub(crate) args: String,
    pub(crate) lifecycle: Arc<RunLifecycle>,
    pub(crate) log: SharedLog,
}

impl SessionCore {
    /// Builds one session's state; the returned receiver is the raw delta
    /// stream the supervisor drains.
    pub(crate) fn new(seed: SessionSeed) -> (Arc<Self>, mpsc::UnboundedReceiver<StreamDelta>) {
        let (wait_frames, _) = broadcast::channel(INPUT_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (deltas, _) = broadcast::channel(DELTA_CAPACITY);
        let (errors, _) = broadcast::channel(ERROR_CAPACITY);
        let (delta_source, raw_deltas) = mpsc::unbounded_channel();
        let core = Arc::new(Self {
            id: seed.id,
            agent: seed.agent,
            source: seed.source,
            prompt_path: seed.prompt_path,
            args: seed.args,
            lifecycle: seed.lifecycle,
            waits: Arc::new(WaitRegistry::new()),
            wait_frames,
            events,
            deltas,
            errors,
            delta_source,
            rounds: AtomicU64::new(0),
            next_index: AtomicU64::new(0),
            runs: Mutex::new(Vec::new()),
            state: watch::Sender::new(SessionState::Alive),
            log: seed.log,
        });
        (core, raw_deltas)
    }

    /// The runs in launch order.
    pub(crate) fn run_ids(&self) -> Vec<LogRunId> {
        self.runs().clone()
    }

    /// Records a run the log has opened for this session.
    pub(crate) fn record_run(&self, run: LogRunId) {
        self.runs().push(run);
    }

    fn runs(&self) -> MutexGuard<'_, Vec<LogRunId>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The state after cancel or close is requested.
    pub(crate) fn interrupted(&self) {
        self.state.send_modify(|state| *state = state.interrupted());
    }

    /// The state after the run reports `Done`.
    pub(crate) fn done(&self) {
        self.state.send_modify(|state| *state = state.done());
    }

    /// The state of a fresh run.
    pub(crate) fn alive(&self) {
        self.state.send_replace(SessionState::Alive);
    }

    /// Installs and retains the next run's fresh cancel handle.
    pub(crate) fn arm_cancel(&self, run: RunId) -> promptforge_api_types::cancel::CancelHandle {
        self.lifecycle.arm(run)
    }

    /// Cancels the run selected by a reducer effect.
    pub(crate) fn cancel_current_run(&self) {
        self.lifecycle.cancel_current();
    }

    /// Clears the lifecycle identity after a run ends.
    pub(crate) fn finish_run(&self, run: RunId) {
        self.lifecycle.finish(run);
    }

    /// Reports one operator-facing failure of `kind` with its display
    /// `message`. No receiver means no client is attached; reports are
    /// ephemeral by design.
    pub(crate) fn report(&self, kind: FailureKind, message: String) {
        let _ = self.errors.send(SessionFailure { kind, message });
    }

    /// Stamps one raw delta with the current round and broadcasts it.
    pub(crate) fn publish_delta(&self, delta: StreamDelta) {
        let (kind, content) = match delta {
            StreamDelta::Text(text) => (DeltaKind::Text, text),
            StreamDelta::Reasoning(text) => (DeltaKind::Reasoning, text),
            // The enum is non-exhaustive across the crate seam; a future
            // side channel has no delta kind yet and stays live-only.
            _ => return,
        };
        // No receiver means no client is attached; deltas are ephemeral
        // and the completed-reply event is the repair, so the drop is the
        // design.
        let _ = self.deltas.send(Delta {
            kind,
            content,
            reply: self.rounds.load(Ordering::SeqCst),
        });
    }

    /// Applies one run event: the side effects first, then the broadcast
    /// under the next transcript index, so a client woken by the event
    /// reads a settled round count.
    pub(crate) fn observe(&self, event: &Event) {
        match event {
            // A failed model round or tool dispatch is operator-visible:
            // the program survives it (the built-in chat pcalls
            // models.loop and returns to waiting), so the run never fails
            // and only the session can tell the client. Both are terminal
            // for the turn.
            Event::ModelTurnFailed { section, .. } | Event::ToolCallFailed { section, .. } => {
                let (kind, boundary) = match event {
                    Event::ModelTurnFailed { .. } => {
                        (FailureKind::ModelTurnFailed, "Model turn failed")
                    }
                    _ => (FailureKind::ToolCallFailed, "Tool call failed"),
                };
                self.lifecycle.settle_current_turn();
                self.report(kind, format!("{boundary} in agent `{section}`"));
            }
            Event::AssistantReply { .. } => self.lifecycle.settle_current_turn(),
            _ => {}
        }
        // The sink is called from one task at a time (the run's loop), so
        // the load-stamp-store is not raced; the deltas only read.
        let mut rounds = self.rounds.load(Ordering::SeqCst);
        let reply = reply_stamp(event, &mut rounds);
        self.rounds.store(rounds, Ordering::SeqCst);
        let index = self.next_index.fetch_add(1, Ordering::SeqCst);
        // The log serialized this same event a moment ago, so this cannot
        // fail; `Null` keeps the index sequence whole if it ever did.
        let event = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
        let _ = self.events.send(SessionEvent {
            index,
            reply,
            event,
        });
    }
}

/// The reply-id rule, applied identically live and on replay: the
/// model-round content kinds are stamped with the current round count,
/// and a reply or tool-call batch advances it.
#[must_use]
pub fn reply_stamp(event: &Event, rounds_seen: &mut u64) -> Option<u64> {
    match event {
        Event::Thinking { .. } => Some(*rounds_seen),
        Event::AssistantReply { .. } | Event::AssistantToolCalls { .. } => {
            let round = *rounds_seen;
            *rounds_seen += 1;
            Some(round)
        }
        _ => None,
    }
}
