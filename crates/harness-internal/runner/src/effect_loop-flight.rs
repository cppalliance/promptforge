//! The effects in flight: each chat, tool-call, and timer effect's
//! performer future, polled inside the run's own future beside the rest.
//!
//! Every future sits in one `FuturesUnordered` as an `Abortable` future
//! under `catch_unwind`, keyed by its effect id, the provenance the effect
//! was issued under, and whether it is a tool call that survives stops. A
//! future that lands yields its effect's answer, and one that panics
//! yields `Dropped` with the panic logged. An aborted future leaves the
//! keyed set at once, since the loop answers its effect as it aborts it,
//! and is torn down the next time the set is polled; whatever it yields
//! then is discarded, so the run never sees two answers for one effect.

use std::any::Any;
use std::collections::HashMap;
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

/// Which effects in flight an abort reaches.
#[derive(Clone, Copy, Debug)]
pub(super) enum Reach {
    /// Every effect in flight: a cancel.
    All,
    /// Every effect but the tool calls that survive stops: a stop.
    AllButSurvivors,
}

/// One effect in flight: what the loop needs to answer or drop it.
struct InFlight {
    /// The provenance the effect was issued under, for its answer record.
    provenance: Provenance,
    /// Aborts the effect's future.
    abort: AbortHandle,
    /// Whether the effect is a tool call that survives stops, which a
    /// stop leaves alone.
    survives_stop: bool,
}

/// The effects in flight and their futures.
pub(super) struct Flights {
    futures: FuturesUnordered<BoxFuture<Landing>>,
    /// The effects still owed an answer. A landing for an id not here is
    /// an aborted future's late end and is discarded.
    effects: HashMap<EffectId, InFlight>,
}

impl Flights {
    pub(super) fn new() -> Self {
        Self {
            futures: FuturesUnordered::new(),
            effects: HashMap::new(),
        }
    }

    /// Whether no effect is owed an answer.
    pub(super) fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// How many effects are owed an answer.
    pub(super) fn len(&self) -> usize {
        self.effects.len()
    }

    /// Puts `answer`, the performer future of effect `id`, in flight under
    /// `provenance`, inside a span that records the effect id, the task
    /// path, and the task-local sequence, so a run's effects trace as a
    /// group. `survives_stop` marks a tool call that survives stops.
    pub(super) fn start(
        &mut self,
        id: EffectId,
        provenance: Provenance,
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
                survives_stop,
            },
        );
    }

    /// Polls for the next effect whose future landed, with its provenance
    /// and answer. A future that panicked lands as `Dropped`, and the
    /// panic is logged against its effect.
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

    /// Aborts every effect in flight that `reach` takes and returns each
    /// one's id and provenance in effect order, for the loop to answer
    /// `Dropped`.
    pub(super) fn abort(&mut self, reach: Reach) -> Vec<(EffectId, Provenance)> {
        let mut chosen: Vec<EffectId> = self
            .effects
            .iter()
            .filter(|(_, in_flight)| match reach {
                Reach::All => true,
                Reach::AllButSurvivors => !in_flight.survives_stop,
            })
            .map(|(id, _)| *id)
            .collect();
        chosen.sort_by_key(|id| id.get());
        let mut aborted = Vec::with_capacity(chosen.len());
        for id in chosen {
            if let Some(in_flight) = self.effects.remove(&id) {
                in_flight.abort.abort();
                aborted.push((id, in_flight.provenance));
            }
        }
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
