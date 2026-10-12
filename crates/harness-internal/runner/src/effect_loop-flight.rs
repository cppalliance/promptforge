//! The effects in flight: each chat, tool-call, and timer effect's
//! performer future, polled inside the run's own future beside the rest,
//! and the tool calls waiting for a slot.
//!
//! Every future in flight sits in one `FuturesUnordered` as an `Abortable`
//! future under `catch_unwind`, keyed by its effect id, the provenance the
//! effect was issued under, whether it is a tool call, and whether it
//! survives stops. A future that lands yields its effect's answer, and one
//! that panics yields `Dropped` with the panic logged. An aborted future
//! leaves the keyed set at once, since the loop answers its effect as it
//! aborts it, and is torn down the next time the set is polled; whatever
//! it yields then is discarded, so the run never sees two answers for one
//! effect.
//!
//! At most `cap` tool calls are in flight at once; chat and timer effects
//! never wait and never count toward it. A tool call beyond the cap waits
//! as its performer future, not yet polled, so a waiting call has started
//! nothing. Whenever a slot frees, because a call landed, panicked, or was
//! aborted, the waiting call with the lowest effect id starts. Ids follow
//! issue order across the run, so the oldest batch finishes first, and a
//! new call waits behind every call already waiting, so none starves.
//!
//! An abort takes waiting calls by the same reach as calls in flight: a
//! cancel takes every effect, a stop every effect but the tool calls that
//! survive stops, and a withdrawal the tool calls a step withdrew. A
//! waiting call it takes is dropped unstarted, and the loop answers it
//! `Dropped` like the rest. Every effect waiting or in flight is owed one
//! answer.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::panic::AssertUnwindSafe;
use std::task::{Context, Poll, Waker};

use futures_util::future::{AbortHandle, Abortable, Aborted, FutureExt as _};
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use promptforge::effect::{EffectAnswer, EffectId};
use promptforge::ids::Provenance;
use tracing::Instrument as _;

use crate::performers::BoxFuture;

/// How one future in flight ended: its answer or its panic's payload, or
/// the abort the loop applied.
type Landing = (EffectId, Result<std::thread::Result<EffectAnswer>, Aborted>);

/// Which held effects an abort reaches.
#[derive(Clone, Copy, Debug)]
pub(super) enum Reach<'a> {
    /// Every effect, waiting or in flight: a cancel.
    All,
    /// Every effect but the tool calls that survive stops: a stop.
    AllButSurvivors,
    /// The listed effects, waiting or in flight: the tool calls a step
    /// withdrew, in effect order.
    Withdrawn(&'a [EffectId]),
}

impl Reach<'_> {
    /// Whether this reach takes effect `id`, which survives stops when
    /// `survives_stop` is set.
    fn takes(self, id: EffectId, survives_stop: bool) -> bool {
        match self {
            Reach::All => true,
            Reach::AllButSurvivors => !survives_stop,
            Reach::Withdrawn(ids) => ids.binary_search(&id).is_ok(),
        }
    }
}

/// One effect in flight: what the loop needs to answer or drop it.
struct InFlight {
    /// The provenance the effect was issued under, for its answer record.
    provenance: Provenance,
    /// Aborts the effect's future.
    abort: AbortHandle,
    /// Whether the effect is a tool call, which counts toward the cap.
    tool_call: bool,
    /// Whether the effect is a tool call that survives stops, which a
    /// stop leaves alone.
    survives_stop: bool,
}

/// A tool call waiting for a slot: its performer future, not yet polled.
struct Waiting {
    provenance: Provenance,
    survives_stop: bool,
    answer: BoxFuture<EffectAnswer>,
}

/// The effects in flight and their futures, and the tool calls waiting
/// for a slot.
pub(super) struct Flights {
    futures: FuturesUnordered<BoxFuture<Landing>>,
    /// The effects in flight still owed an answer. A landing for an id not
    /// here is an aborted future's late end and is discarded.
    effects: HashMap<EffectId, InFlight>,
    /// Tool calls waiting for a slot; the lowest id starts first.
    waiting: BTreeMap<EffectId, Waiting>,
    /// The most tool calls in flight at once.
    cap: NonZeroUsize,
}

impl Flights {
    pub(super) fn new(cap: NonZeroUsize) -> Self {
        Self {
            futures: FuturesUnordered::new(),
            effects: HashMap::new(),
            waiting: BTreeMap::new(),
            cap,
        }
    }

    /// Whether no effect is owed an answer.
    pub(super) fn is_empty(&self) -> bool {
        self.effects.is_empty() && self.waiting.is_empty()
    }

    /// How many effects are owed an answer, waiting calls included.
    pub(super) fn len(&self) -> usize {
        self.effects.len() + self.waiting.len()
    }

    /// Puts `answer`, the performer future of chat or timer effect `id`,
    /// in flight under `provenance`. It never waits and never counts
    /// toward the cap.
    pub(super) fn start(
        &mut self,
        id: EffectId,
        provenance: Provenance,
        answer: BoxFuture<EffectAnswer>,
    ) {
        self.launch(id, provenance, false, false, answer);
    }

    /// Queues `answer`, the performer future of tool call `id`, under
    /// `provenance`, then starts waiting calls, lowest id first, while
    /// fewer than `cap` tool calls are in flight. `survives_stop` marks a
    /// call that survives stops.
    pub(super) fn queue(
        &mut self,
        id: EffectId,
        provenance: Provenance,
        survives_stop: bool,
        answer: BoxFuture<EffectAnswer>,
    ) {
        self.waiting.insert(
            id,
            Waiting {
                provenance,
                survives_stop,
                answer,
            },
        );
        self.fill();
    }

    /// Starts waiting calls, lowest id first, while fewer than `cap` tool
    /// calls are in flight.
    fn fill(&mut self) {
        let mut running = self
            .effects
            .values()
            .filter(|in_flight| in_flight.tool_call)
            .count();
        while running < self.cap.get() {
            let Some((id, call)) = self.waiting.pop_first() else {
                return;
            };
            self.launch(id, call.provenance, true, call.survives_stop, call.answer);
            running += 1;
        }
    }

    /// Puts `answer`, the performer future of effect `id`, in flight under
    /// `provenance`, inside a span that records the effect id, the task
    /// path, and the task-local sequence, so a run's effects trace as a
    /// group. `tool_call` marks a tool call, and `survives_stop` one that
    /// survives stops.
    fn launch(
        &mut self,
        id: EffectId,
        provenance: Provenance,
        tool_call: bool,
        survives_stop: bool,
        answer: BoxFuture<EffectAnswer>,
    ) {
        let span = tracing::info_span!(
            "effect",
            effect = %id,
            task = %provenance.task,
            seq = provenance.seq
        );
        let (abort, registration) = AbortHandle::new_pair();
        let caught = AssertUnwindSafe(answer.instrument(span)).catch_unwind();
        let landing = Abortable::new(caught, registration).map(move |landed| (id, landed));
        self.futures.push(Box::pin(landing));
        self.effects.insert(
            id,
            InFlight {
                provenance,
                abort,
                tool_call,
                survives_stop,
            },
        );
    }

    /// Polls for the next effect whose future landed, with its provenance
    /// and answer. A future that panicked lands as `Dropped`, and the
    /// panic is logged against its effect. A tool call that lands frees
    /// its slot for the next waiting call.
    pub(super) fn poll_landed(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<(EffectId, Provenance, EffectAnswer)> {
        loop {
            let Poll::Ready(Some((id, landed))) = self.futures.poll_next_unpin(cx) else {
                return Poll::Pending;
            };
            let Some(in_flight) = self.effects.remove(&id) else {
                continue;
            };
            if in_flight.tool_call {
                self.fill();
            }
            let answer = match landed {
                Ok(Ok(answer)) => answer,
                Ok(Err(panic)) => {
                    tracing::error!(
                        effect = %id,
                        task = %in_flight.provenance.task,
                        panic = panic_message(&*panic),
                        "a performer panicked; its effect is dropped"
                    );
                    EffectAnswer::Dropped
                }
                // Only an aborted effect's future yields `Aborted`, and an
                // aborted effect left `effects` as it was aborted.
                Err(Aborted) => EffectAnswer::Dropped,
            };
            return Poll::Ready((id, in_flight.provenance, answer));
        }
    }

    /// The next effect that has already landed, without waiting. Polling
    /// also tears down every aborted future the set still holds.
    pub(super) fn landed_now(&mut self) -> Option<(EffectId, Provenance, EffectAnswer)> {
        match self.poll_landed(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(landed) => Some(landed),
            Poll::Pending => None,
        }
    }

    /// Takes every effect waiting or in flight that `reach` takes:
    /// drops each waiting call's future unstarted, aborts each effect in
    /// flight, and returns each one's id and provenance in effect order,
    /// for the loop to answer `Dropped`. Then starts waiting calls in the
    /// slots it freed, so it never starts a call the same reach takes.
    pub(super) fn abort(&mut self, reach: Reach<'_>) -> Vec<(EffectId, Provenance)> {
        let in_flight = self
            .effects
            .iter()
            .filter(|(id, in_flight)| reach.takes(**id, in_flight.survives_stop))
            .map(|(id, _)| *id);
        let waiting = self
            .waiting
            .iter()
            .filter(|(id, call)| reach.takes(**id, call.survives_stop))
            .map(|(id, _)| *id);
        let mut chosen: Vec<EffectId> = in_flight.chain(waiting).collect();
        chosen.sort_unstable();
        let mut aborted = Vec::with_capacity(chosen.len());
        for id in chosen {
            if let Some(in_flight) = self.effects.remove(&id) {
                in_flight.abort.abort();
                aborted.push((id, in_flight.provenance));
            } else if let Some(call) = self.waiting.remove(&id) {
                aborted.push((id, call.provenance));
            }
        }
        self.fill();
        aborted
    }
}

/// A panic payload's message, when it carries one.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic with no message")
}
