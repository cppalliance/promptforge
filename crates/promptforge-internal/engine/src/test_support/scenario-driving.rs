//! A started scenario: the [`Driving`] run a test steps by hand or to its
//! end, the [`Trace`] it leaves, and the [`Record`] form its events take
//! for the suites' assertions.

use std::sync::Mutex;
use std::thread::JoinHandle;

use promptforge_types::event::Event;
use promptforge_vfs::VfsError;

use super::super::recording::{Observation, Observer};
use super::super::{drive_step, forward};
use super::{Answers, TestStore};
use crate::execute::{
    AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Run, RunError, RunResult,
};

/// A started scenario, stepped by hand or to its end.
pub(crate) struct Driving {
    /// The run, until it is done or when the prepare refused it.
    pub(super) run: Option<Run>,
    pub(super) result: Option<RunResult>,
    pub(super) answers: Answers,
    /// The parse events, then every event the run reported, in order.
    pub(super) events: Vec<Event>,
    pub(super) effects: Vec<(EffectRecord, AnswerRecord)>,
    /// The effects the scenario holds, unanswered until the run decides.
    pub(super) held: Vec<(EffectId, Effect)>,
    pub(super) store: TestStore,
    /// The thread that cancels the run on the scenario's signal.
    pub(super) canceller: Option<JoinHandle<()>>,
}

impl Driving {
    /// Steps the run once, answering every effect the step issued that the
    /// scenario does not hold, and returns whether stepping again can move
    /// the run: `false` once it is done, and while it waits only on held
    /// effects.
    pub(crate) fn step(&mut self) -> bool {
        let Some(run) = self.run.as_mut() else {
            return false;
        };
        let answers = &self.answers;
        let effects = &mut self.effects;
        let mut moved = false;
        let mut perform = |_: EffectId, effect: &Effect| answers.answer(effect);
        let mut answered = |effect: &Effect, answer: &EffectAnswer| {
            moved = true;
            effects.push((effect.record(), answer.record()));
        };
        let holding = self.held.len();
        let stepped = drive_step(
            run,
            &mut perform,
            &mut answered,
            &mut self.held,
            &mut self.events,
        );
        let Some(result) = stepped else {
            return moved || self.held.len() > holding;
        };
        self.result = Some(result);
        self.run = None;
        false
    }

    /// Sets the run's cancel flag between steps, as a caller cancelling a
    /// run it steps would. A run that is done ignores it.
    pub(crate) fn cancel(&mut self) {
        if let Some(run) = self.run.as_mut() {
            run.cancel();
        }
    }

    /// Steps the run to its end and returns what it produced.
    ///
    /// # Panics
    /// When the run waits only on held effects, which nothing releases
    /// until the run decides.
    #[track_caller]
    pub(crate) fn finish(mut self) -> Trace {
        while self.step() {}
        let Some(result) = self.result else {
            panic!(
                "the run waits only on {} held effects, and no cancel has decided it",
                self.held.len()
            );
        };
        // Dropping the answers hangs up on a canceller whose signal never
        // came, so the join below returns.
        drop(self.answers);
        if let Some(canceller) = self.canceller {
            canceller
                .join()
                .expect("the canceller thread does not panic");
        }
        Trace {
            result,
            events: self.events,
            effects: self.effects,
            store: self.store,
        }
    }
}

/// What a scenario's run produced: its result, its events, every effect
/// it issued paired with its answer, and its store.
pub(crate) struct Trace {
    result: RunResult,
    /// The parse events, then every event the run reported, in order.
    events: Vec<Event>,
    effects: Vec<(EffectRecord, AnswerRecord)>,
    store: TestStore,
}

impl Trace {
    /// The run's result.
    pub(crate) fn result(&self) -> &RunResult {
        &self.result
    }

    /// The text of a run that succeeded.
    ///
    /// # Panics
    /// When the run did not succeed, naming its result.
    #[track_caller]
    pub(crate) fn text(&self) -> &str {
        match &self.result {
            RunResult::Ok(text) => text,
            other => panic!("expected the run to succeed, got {other:?}"),
        }
    }

    /// The error of a run that failed.
    ///
    /// # Panics
    /// When the run did not fail, naming its result.
    #[track_caller]
    pub(crate) fn error(&self) -> &RunError {
        match &self.result {
            RunResult::Failure(error) => error,
            other => panic!("expected the run to fail, got {other:?}"),
        }
    }

    /// The parse events, then every event the run reported, in order.
    pub(crate) fn events(&self) -> &[Event] {
        &self.events
    }

    /// Every effect the run issued, beside its answer, in the order the
    /// answers went, which is issue order for every effect the scenario
    /// does not hold. An effect issued after the run decided its outcome,
    /// and a held one, is paired with [`AnswerRecord::Dropped`].
    pub(crate) fn effects(&self) -> &[(EffectRecord, AnswerRecord)] {
        &self.effects
    }

    /// Replays the events onto `observer`, parse events first.
    pub(crate) fn replay(&self, observer: &dyn Observer) {
        forward(self.events.clone(), observer);
    }

    /// The events as the suites' observer sees them, parse events first.
    pub(crate) fn records(&self) -> Vec<Record> {
        let recorder = Recorder::default();
        self.replay(&recorder);
        recorder
            .0
            .into_inner()
            .expect("the record lock is not poisoned")
    }

    /// Reads a store path after the run, through a fresh access.
    pub(crate) fn read(&self, path: &str) -> Result<String, VfsError> {
        self.store.read(path)
    }
}

/// One observation: which execution and section emitted it, plus the
/// rendered event detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) execution: String,
    pub(crate) section: String,
    pub(crate) detail: String,
}

impl Record {
    /// Builds an expected record from borrowed parts, for assertions.
    pub(crate) fn new(execution: &str, section: &str, detail: &str) -> Record {
        Record {
            execution: execution.to_owned(),
            section: section.to_owned(),
            detail: detail.to_owned(),
        }
    }
}

/// The observer [`Trace::records`] replays the events onto.
#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the record lock is not poisoned")
            .push(Record::new(execution, section, &event.to_string()));
    }
}
