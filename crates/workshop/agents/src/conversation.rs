//! One conversation: the handle a socket holds, and the state that
//! outlives any socket connection.
//!
//! A conversation owns one agent's one run. Its transcript is every event
//! the run has reported, numbered from zero and held in memory. A live
//! subscriber receives each event as the conversation observes it, and a
//! reconnecting client reads [`Conversation::transcript`] past its last
//! seen index. Deltas go out on their own broadcast, stamped with their
//! round's id, and never enter the transcript. The conversation's
//! unresolved input waits survive a client's disconnect. Its channels
//! close when the run ends.

#[path = "conversation-run.rs"]
mod run;

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use harness::RunControl;
use harness::record::RunId;
use promptforge::event::Event;
use tokio::sync::{broadcast, watch};

use crate::input::{WaitError, WaitFrame, WaitRegistry, complete_input_response};
use crate::protocol::{ConversationId, Delta, SessionEvent};
use crate::state::{FailureKind, SessionFailure, SessionState};
use crate::table::{self, Table};
use crate::transcript::Transcript;

/// Capacity of a conversation's event broadcast. The broadcast is the
/// wakeup; a receiver that lags repairs by reading the transcript past
/// its cursor.
const EVENT_CAPACITY: usize = 256;

/// Capacity of a conversation's delta broadcast. Deltas are ephemeral: a
/// receiver that lags loses chunks, and the completed-reply event is the
/// repair path.
const DELTA_CAPACITY: usize = 256;

/// Capacity of a conversation's input-frame broadcast. A conversation
/// holds at most a handful of waits; the registry's retained state is the
/// durable-delivery repair path on lag.
const INPUT_CAPACITY: usize = 32;

/// Capacity of a conversation's failure broadcast. Failures are rare
/// one-off reports, and a receiver that lags misses only what the durable
/// transcript already shows as a turn without a reply.
const ERROR_CAPACITY: usize = 8;

/// A running agent conversation: the handle a socket launches, answers,
/// stops, closes, and subscribes through. Cheap to clone; every clone
/// names the same conversation.
#[derive(Clone)]
pub struct Conversation {
    core: Arc<Core>,
}

impl fmt::Debug for Conversation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Conversation")
            .field("id", &self.core.id)
            .field("agent", &self.core.agent)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

/// The state behind every handle of one conversation.
struct Core {
    /// The Workshop-minted id, the run's name.
    id: ConversationId,
    /// The agent the conversation runs, as the agent menu lists it.
    agent: String,
    /// Every event the run has reported, stamped as the live stream
    /// stamps it.
    transcript: Transcript,
    /// The unresolved user-input waits.
    waits: Arc<WaitRegistry>,
    /// The live senders, until the conversation ends and drops them.
    channels: Mutex<Option<Channels>>,
    /// Where the run stands.
    state: watch::Sender<SessionState>,
    /// The run the recorder began for this conversation.
    run: Mutex<Option<RunId>>,
    /// The control over the run, once the run is handed its Harness.
    control: Mutex<Option<RunControl>>,
    /// The table the conversation leaves when it ends.
    table: Weak<Table>,
}

/// A conversation's live senders.
#[derive(Clone)]
struct Channels {
    events: broadcast::Sender<SessionEvent>,
    deltas: broadcast::Sender<Delta>,
    waits: broadcast::Sender<WaitFrame>,
    errors: broadcast::Sender<SessionFailure>,
}

/// A receiver whose sender is already gone, for a subscription made
/// after the conversation ended.
fn closed<T: Clone>() -> broadcast::Receiver<T> {
    broadcast::channel(1).1
}

impl Conversation {
    /// A conversation with nothing reported yet, which leaves `table`
    /// when it ends.
    pub(crate) fn new(id: ConversationId, agent: String, table: Weak<Table>) -> Self {
        let channels = Channels {
            events: broadcast::channel(EVENT_CAPACITY).0,
            deltas: broadcast::channel(DELTA_CAPACITY).0,
            waits: broadcast::channel(INPUT_CAPACITY).0,
            errors: broadcast::channel(ERROR_CAPACITY).0,
        };
        Self {
            core: Arc::new(Core {
                id,
                agent,
                transcript: Transcript::new(),
                waits: Arc::new(WaitRegistry::new()),
                channels: Mutex::new(Some(channels)),
                state: watch::Sender::new(SessionState::Alive),
                run: Mutex::new(None),
                control: Mutex::new(None),
                table,
            }),
        }
    }

    /// The conversation's id.
    #[must_use]
    pub fn id(&self) -> &ConversationId {
        &self.core.id
    }

    /// The agent the conversation runs.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.core.agent
    }

    /// Where the run stands: [`SessionState::Alive`] until a close,
    /// [`SessionState::Closing`] while a close drains the run, and
    /// [`SessionState::Closed`] once the run has ended.
    #[must_use]
    pub fn state(&self) -> SessionState {
        *self.core.state.borrow()
    }

    /// The run the recorder began for this conversation, once it has.
    #[must_use]
    pub fn run_id(&self) -> Option<RunId> {
        *lock(&self.core.run)
    }

    /// Subscribes to the conversation's events from this call on.
    /// Subscribe first, then read [`Conversation::transcript`] past the
    /// last seen index, and skip live events below the cursor: nothing is
    /// lost between the two.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<SessionEvent> {
        self.channels()
            .map_or_else(closed, |channels| channels.events.subscribe())
    }

    /// Subscribes to the conversation's live deltas from this call on.
    #[must_use]
    pub fn subscribe_deltas(&self) -> broadcast::Receiver<Delta> {
        self.channels()
            .map_or_else(closed, |channels| channels.deltas.subscribe())
    }

    /// Subscribes to the conversation's input-wait frames from this call
    /// on. Read [`Conversation::unresolved_waits`] after subscribing to
    /// learn of waits already open.
    #[must_use]
    pub fn subscribe_waits(&self) -> broadcast::Receiver<WaitFrame> {
        self.channels()
            .map_or_else(closed, |channels| channels.waits.subscribe())
    }

    /// Subscribes to the conversation's failure reports from this call
    /// on. Ephemeral like the deltas.
    #[must_use]
    pub fn subscribe_errors(&self) -> broadcast::Receiver<SessionFailure> {
        self.channels()
            .map_or_else(closed, |channels| channels.errors.subscribe())
    }

    /// The transcript from index `from` on, read from memory. Each entry
    /// carries the index and reply stamp the live
    /// [`Conversation::subscribe_events`] stream gave the same event.
    #[must_use]
    pub fn transcript(&self, from: u64) -> Vec<SessionEvent> {
        self.core.transcript.since(from)
    }

    /// The unresolved wait tokens in creation order: the teardown leak
    /// probe, empty once the run has ended.
    #[must_use]
    pub fn unresolved_waits(&self) -> Vec<String> {
        self.core.waits.unresolved()
    }

    /// Answers the wait holding `token` with the operator's `text`.
    /// `before_resume` runs after the answer is accepted and before the
    /// suspended ask resumes, for a client's own turn bookkeeping.
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
        complete_input_response(&self.core.waits, token, text, before_resume)
    }

    /// Stops the round in flight: the run drops every effect except a
    /// question to the operator, and goes on. An open question stays
    /// open, and its token still answers it. Nothing happens before the
    /// run is handed its Harness.
    pub fn stop_round(&self) {
        if let Some(control) = lock(&self.core.control).as_ref() {
            control.stop_round();
        }
    }

    /// Cancels the run for good: the state is `Closing` at once, every
    /// effect in flight is answered `Dropped`, open waits die as
    /// cancelled, and the state is `Closed` once the run has ended. A
    /// close before the run is handed its Harness ends that run before it
    /// begins. Idempotent.
    pub fn close(&self) {
        let control = lock(&self.core.control);
        self.core
            .state
            .send_modify(|state| *state = state.interrupted());
        if let Some(control) = control.as_ref() {
            control.cancel();
        }
    }

    /// Resolves once a close has been requested.
    pub async fn closing(&self) {
        let mut state = self.core.state.subscribe();
        // The sender lives in the core this handle holds, so the watch
        // never closes while this waits.
        let _ = state.wait_for(|state| *state != SessionState::Alive).await;
    }

    /// The live senders, or `None` once the conversation has ended.
    fn channels(&self) -> Option<Channels> {
        lock(&self.core.channels).clone()
    }

    /// Notes the run the recorder began.
    pub(crate) fn note_run(&self, run: RunId) {
        *lock(&self.core.run) = Some(run);
    }

    /// Reports one operator-facing failure of `kind` with its display
    /// `message`. No receiver means no client is attached; reports are
    /// ephemeral by design.
    fn report(&self, kind: FailureKind, message: String) {
        if let Some(channels) = self.channels() {
            let _ = channels.errors.send(SessionFailure { kind, message });
        }
    }

    /// Applies one recorded event: a failed model round or tool dispatch
    /// is reported first, then the event is appended to the transcript
    /// under the next index and that same entry is broadcast.
    pub(crate) fn observe(&self, event: &Event) {
        // The program survives a failed round or tool dispatch (the
        // built-in chat pcalls `models.loop` and returns to waiting), so
        // the run never fails and only the conversation can tell the
        // client.
        match event {
            Event::ModelTurnFailed { section, .. } => self.report(
                FailureKind::ModelTurnFailed,
                format!("Model turn failed in agent `{section}`"),
            ),
            Event::ToolCallFailed { section, .. } => self.report(
                FailureKind::ToolCallFailed,
                format!("Tool call failed in agent `{section}`"),
            ),
            _ => {}
        }
        let entry = self.core.transcript.push(event);
        if let Some(channels) = self.channels() {
            // No receiver means no client is attached; the transcript
            // already holds the entry, so a late client reads it there.
            let _ = channels.events.send(entry);
        }
    }

    /// Ends the conversation once its run has: the state is `Closed`, the
    /// channels close, and the conversation leaves its table.
    fn end(&self) {
        self.core.state.send_modify(|state| *state = state.done());
        lock(&self.core.channels).take();
        if let Some(table) = self.core.table.upgrade() {
            table::lock(&table).remove(&self.core.id);
        }
    }
}

/// Locks one of the core's slots. Every write is one store, so a poisoned
/// lock still holds a usable value.
fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}
