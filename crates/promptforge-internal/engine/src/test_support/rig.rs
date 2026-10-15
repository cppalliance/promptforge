//! The scheduler-level test driver. A [`Rig`] starts a [`Scenario`]'s run
//! and steps it on the scenario's own stepper, answering every effect the
//! scenario does not hold the moment it is issued, and keeps what a
//! run-level test never reads: each issued effect's record and each model
//! round, in issue order, and the run itself, so a test reaches the
//! scheduler between steps and reads its task slots once the run is done.
//! Between steps a test releases the held effects it chooses, in the order
//! it chooses, or cancels the run; a shuffled rig answers each step's
//! effects in a seeded permutation instead of issue order.
//! The scenario fills in the run's model set and tool set and chooses its
//! store and limits; a test that observes the run replays the trace.

use promptforge_types::event::Event;
use promptforge_types::ids::TaskId;

use super::scenario::{Driving, Scenario, Trace};
use super::seeded::Seeded;
use crate::execute::scheduler::Scheduler;
use crate::execute::scheduler::test_hooks::TaskState;
use crate::execute::{Effect, EffectRecord, Round, Run};

/// What the rig keeps of the effects a run issues, in issue order.
#[derive(Default)]
struct Tap {
    /// Every issued effect's record, performed, held, or dropped at issue.
    issued: Vec<EffectRecord>,
    /// The round of every issued `Chat` effect.
    rounds: Vec<Round>,
}

impl Tap {
    fn record(&mut self, effect: &Effect) {
        self.issued.push(effect.record());
        if let Effect::Chat { round, .. } = effect {
            self.rounds.push(*round);
        }
    }
}

/// A scenario's run on the rig, stepped by hand or to its end.
pub(crate) struct Rig {
    driving: Driving,
    tap: Tap,
    /// The permutation each step's answers go in, when the rig shuffles.
    shuffle: Option<Seeded>,
}

impl Rig {
    /// Parses the scenario's source, prepares its run, and starts it on
    /// the rig.
    ///
    /// # Panics
    /// When the source fails to parse, naming the error.
    #[track_caller]
    pub(crate) fn start(scenario: Scenario) -> Rig {
        Rig {
            driving: scenario.start(),
            tap: Tap::default(),
            shuffle: None,
        }
    }

    /// Drives the scenario's run to its end on the rig and returns what it
    /// left.
    ///
    /// # Panics
    /// When the source fails to parse, or the run waits only on held
    /// effects.
    #[track_caller]
    pub(crate) fn run(scenario: Scenario) -> RigTrace {
        Rig::start(scenario).finish()
    }

    /// [`run`](Rig::run), answering the effects each step issues in
    /// [`Seeded`]'s permutation under `seed` instead of issue order, so
    /// one seed is one completion interleaving.
    ///
    /// # Panics
    /// As [`run`](Rig::run) does.
    #[track_caller]
    pub(crate) fn shuffled(scenario: Scenario, seed: u64) -> RigTrace {
        let mut rig = Rig::start(scenario);
        rig.shuffle = Some(Seeded::new(seed));
        rig.finish()
    }

    /// Steps the run once, as the scenario steps it, tapping each effect
    /// the step issues, and returns whether stepping again can move it.
    pub(crate) fn step(&mut self) -> bool {
        let tap = &mut self.tap;
        let shuffle = &mut self.shuffle;
        self.driving.step_ordered(
            |effect| tap.record(effect),
            |wave| {
                if let Some(seeded) = shuffle {
                    seeded.shuffle(wave);
                }
            },
        )
    }

    /// Steps the run until stepping cannot move it: it is done, or it
    /// waits only on held effects.
    #[track_caller]
    pub(crate) fn settle(&mut self) {
        while self.step() {}
    }

    /// Answers the first held effect `which` matches, in issue order, as
    /// the scenario answers an effect it does not hold. The run sees the
    /// answer at its next step.
    ///
    /// # Panics
    /// When no held effect matches.
    #[track_caller]
    pub(crate) fn release(&mut self, which: impl Fn(&Effect) -> bool) {
        self.driving.release(which);
    }

    /// The parse events, then every event the run reported so far, for a
    /// test that steps until an event appears.
    pub(crate) fn events(&self) -> &[Event] {
        self.driving.events()
    }

    /// Sets the run's cancel flag between steps.
    pub(crate) fn cancel(&mut self) {
        self.driving.cancel();
    }

    /// The run itself, for a test that hands it an answer by hand.
    ///
    /// # Panics
    /// When the prepare refused the run.
    #[track_caller]
    pub(crate) fn run_for_test(&mut self) -> &mut Run {
        self.driving
            .run_mut()
            .expect("the prepare accepted the run")
    }

    /// The scheduler behind the run, for a test that reaches below the run
    /// between steps.
    ///
    /// # Panics
    /// When the prepare refused the run, which then has no scheduler.
    #[track_caller]
    pub(crate) fn scheduler_for_test(&mut self) -> &mut Scheduler {
        self.run_for_test().scheduler_for_test()
    }

    /// Steps the run to its end and returns what it left.
    ///
    /// # Panics
    /// When the run waits only on held effects, which nothing releases
    /// until the run decides.
    #[track_caller]
    pub(crate) fn finish(mut self) -> RigTrace {
        self.settle();
        let (trace, run) = self.driving.end();
        RigTrace {
            trace,
            run,
            tap: self.tap,
        }
    }
}

/// What a run on the rig left: the scenario's trace, the effects and
/// rounds it issued in issue order, and the finished run.
pub(crate) struct RigTrace {
    trace: Trace,
    /// The finished run, or `None` when the prepare refused it.
    run: Option<Run>,
    tap: Tap,
}

impl RigTrace {
    /// The run's result, events, answered effects, and store.
    pub(crate) fn trace(&self) -> &Trace {
        &self.trace
    }

    /// Every effect the run issued, in issue order, whether it was
    /// performed, held, or dropped at issue.
    pub(crate) fn issued(&self) -> &[EffectRecord] {
        &self.tap.issued
    }

    /// The round of every `Chat` effect the run issued, in issue order.
    pub(crate) fn rounds(&self) -> &[Round] {
        &self.tap.rounds
    }

    /// The state of `task`'s slot at the run's end, or `None` when no task
    /// with that id was started.
    ///
    /// # Panics
    /// When the prepare refused the run, which then has no scheduler.
    #[track_caller]
    pub(crate) fn task_state_for_test(&mut self, task: &TaskId) -> Option<TaskState> {
        self.run
            .as_mut()
            .expect("the prepare accepted the run")
            .scheduler_for_test()
            .task_state_for_test(task)
    }
}
