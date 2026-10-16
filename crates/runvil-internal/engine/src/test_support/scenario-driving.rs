//! A started scenario: the [`Driving`] run a test steps by hand or to its
//! end, the [`Answers`] it gives the run's effects, the [`TestStore`] the
//! run writes, the [`Trace`] it leaves, and the [`Record`] and
//! [`Capture`] forms its events take for the suites' assertions.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use runvil_types::event::Event;
use runvil_types::tools::{ToolError, ToolId};
use runvil_vfs::{Origin, VfsError, VfsRef};

use super::super::FnTool;
use super::super::recording::{
    DebugCapture, DebugEvent, NullObserver, Observation, Observer, forward_one,
};
use super::super::{drive_step, forward};
use super::{Chat, Matcher};
use crate::execute::{
    AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Run, RunError, RunResult,
};
use crate::lua::run_store_op;
use crate::model::{CompletionError, CompletionErrorKind};

/// How a scenario answers its run's effects: a store operation on the
/// effect's own access, a timer at once, a tool call through the
/// scenario's tool of that id, and a model round from its script or
/// closure, unless the scenario holds the effect.
pub(super) struct Answers {
    pub(super) tools: BTreeMap<ToolId, FnTool>,
    pub(super) chat: Option<Chat>,
    pub(super) hold: Option<Matcher>,
    /// Which effect wakes the canceller, and the wake.
    pub(super) signal: Option<(Matcher, Sender<()>)>,
    /// Whether the canceller was woken.
    pub(super) signalled: Cell<bool>,
    pub(super) timeout: Duration,
}

impl Answers {
    /// Answers `effect` at once, or `None` when the scenario holds it,
    /// waking the canceller either way when `effect` is its signal.
    fn answer(&self, effect: &Effect) -> Option<EffectAnswer> {
        let held = self.hold.as_ref().is_some_and(|held| held(effect));
        let answer = (!held).then(|| self.perform(effect));
        if let Some((signal, wake)) = &self.signal
            && signal(effect)
        {
            // Only the first signal wakes the canceller; a later send
            // finds it gone.
            let _ = wake.send(());
            self.signalled.set(true);
        }
        answer
    }

    /// The answer `effect` gets when the scenario does not hold it. A call
    /// to a tool the scenario lacks is refused, and so is a model round
    /// when the scenario has no model.
    fn perform(&self, effect: &Effect) -> EffectAnswer {
        match effect {
            Effect::Chat {
                messages,
                tools,
                options,
                ..
            } => match &self.chat {
                Some(Chat::Script(chat)) => {
                    let answer = chat.complete_now(messages, tools, options, self.timeout);
                    EffectAnswer::Chat(answer.map(Box::new))
                }
                Some(Chat::Closure(answer)) => answer(effect),
                None => EffectAnswer::Chat(Err(CompletionError::new(
                    CompletionErrorKind::Unavailable,
                    "model access is turned off or not configured",
                ))),
            },
            Effect::ToolCall { tool, args, .. } => match self.tools.get(tool) {
                Some(tool) => EffectAnswer::ToolCall(tool.call_now(args.clone())),
                None => EffectAnswer::ToolCall(Err(ToolError::message(
                    "the tool the call names has no implementation in the test driver's tool table",
                ))),
            },
            Effect::Vfs { access, op } => EffectAnswer::Vfs(run_store_op(access, op.clone())),
            Effect::Timer { .. } => EffectAnswer::Timer,
        }
    }
}

/// A started scenario, stepped by hand or to its end.
pub(crate) struct Driving {
    /// The run, kept once it is done; `None` when the prepare refused it.
    pub(super) run: Option<Run>,
    /// The run's result, once it is done or when the prepare refused it.
    pub(super) result: Option<RunResult>,
    pub(super) answers: Answers,
    /// The parse events, then every event the run reported, in order.
    pub(super) events: Vec<Event>,
    /// How many of the events the parse reported.
    pub(super) parsed: usize,
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
    /// effects with no woken canceller to wait for.
    pub(crate) fn step(&mut self) -> bool {
        self.step_with(|_| {})
    }

    /// [`step`](Driving::step), handing each effect the step issues to
    /// `issued` in issue order, before it is answered or held.
    pub(crate) fn step_with(&mut self, issued: impl FnMut(&Effect)) -> bool {
        self.step_ordered(issued, |_| {})
    }

    /// [`step_with`](Driving::step_with), answering the effects the step
    /// issues in the order `order` leaves them in, in place of issue order.
    pub(crate) fn step_ordered(
        &mut self,
        mut issued: impl FnMut(&Effect),
        order: impl FnOnce(&mut [(EffectId, Effect)]),
    ) -> bool {
        if self.result.is_some() {
            return false;
        }
        let Some(run) = self.run.as_mut() else {
            return false;
        };
        let effects = &mut self.effects;
        let mut moved = false;
        let mut answered = |effect: &Effect, answer: &EffectAnswer| {
            moved = true;
            effects.push((effect.record(), answer.record()));
        };
        let holding = self.held.len();
        let stepped = drive_step(
            run,
            &mut issued,
            &mut |_, _| None,
            &mut answered,
            &mut self.held,
            &mut self.events,
        );
        if let Some(result) = stepped {
            self.result = Some(result);
            return false;
        }
        // A deciding step answered every effect, held ones included, so
        // only an undecided step leaves a wave to answer.
        let mut wave = self.held.split_off(holding.min(self.held.len()));
        order(&mut wave);
        for (id, effect) in wave {
            match self.answers.answer(&effect) {
                Some(answer) => {
                    moved = true;
                    self.effects.push((effect.record(), answer.record()));
                    run.resume(id, answer);
                }
                None => self.held.push((id, effect)),
            }
        }
        moved || self.held.len() > holding || self.await_cancel()
    }

    /// Answers the first held effect `which` matches, in issue order, as
    /// the scenario answers an effect it does not hold.
    ///
    /// # Panics
    /// When no held effect matches.
    #[track_caller]
    pub(crate) fn release(&mut self, which: impl Fn(&Effect) -> bool) {
        let Some(at) = self.held.iter().position(|(_, effect)| which(effect)) else {
            panic!("none of the {} held effects matches", self.held.len());
        };
        let (id, effect) = self.held.remove(at);
        let answer = self.answers.perform(&effect);
        self.effects.push((effect.record(), answer.record()));
        self.run
            .as_mut()
            .expect("a run holding an effect has started")
            .resume(id, answer);
    }

    /// The parse events, then every event the run reported so far, in
    /// order.
    pub(crate) fn events(&self) -> &[Event] {
        &self.events
    }

    /// The run, done or not, or `None` when the prepare refused it.
    pub(crate) fn run_mut(&mut self) -> Option<&mut Run> {
        self.run.as_mut()
    }

    /// Joins the canceller once it is woken, so the next step sees its
    /// cancel, and returns whether it did. A run waiting only on held
    /// effects moves again only through that cancel.
    fn await_cancel(&mut self) -> bool {
        if !self.answers.signalled.get() {
            return false;
        }
        let Some(canceller) = self.canceller.take() else {
            return false;
        };
        canceller
            .join()
            .expect("the canceller thread does not panic");
        true
    }

    /// Sets the run's cancel flag between steps, as a caller cancelling a
    /// run it steps would. A run that is done ignores it.
    pub(crate) fn cancel(&mut self) {
        if self.result.is_none()
            && let Some(run) = self.run.as_mut()
        {
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
        self.end().0
    }

    /// Ends a run already stepped as far as it goes and returns what it
    /// produced, beside the run itself, or `None` when the prepare refused
    /// it.
    ///
    /// # Panics
    /// When the run is not done: it waits only on held effects, which
    /// nothing releases until the run decides.
    #[track_caller]
    pub(crate) fn end(self) -> (Trace, Option<Run>) {
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
        let trace = Trace {
            result,
            events: self.events,
            parsed: self.parsed,
            effects: self.effects,
            store: self.store,
            turns: self.run.as_ref().map_or(0, Run::turns_for_test),
        };
        (trace, self.run)
    }
}

/// What a scenario's run produced: its result, its events, every effect
/// it issued paired with its answer, its store, and its turn count.
pub(crate) struct Trace {
    result: RunResult,
    /// The parse events, then every event the run reported, in order.
    events: Vec<Event>,
    /// How many of the events the parse reported.
    parsed: usize,
    effects: Vec<(EffectRecord, AnswerRecord)>,
    store: TestStore,
    turns: u32,
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

    /// Every event the run reported, in order, without the parse events
    /// ahead of them.
    pub(crate) fn run_events(&self) -> &[Event] {
        &self.events[self.parsed..]
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
        records_of(&self.events)
    }

    /// The run's own events as the suites' observer sees them, without the
    /// parse events: what an observer on the run alone would have seen.
    pub(crate) fn run_records(&self) -> Vec<Record> {
        records_of(self.run_events())
    }

    /// The debug pair of every model turn the run reported, request before
    /// response, in order. The run reports the pair only when its context
    /// turns debug reporting on.
    pub(crate) fn captures(&self) -> Vec<Capture> {
        let captures = Captures::default();
        for event in self.events.iter().cloned() {
            forward_one(event, &NullObserver, Some(&captures));
        }
        captures
            .0
            .into_inner()
            .expect("the capture lock is not poisoned")
    }

    /// The run's model-turn counter at its end: one per round that produced
    /// a reply, an empty one included. Zero for a run that never started.
    pub(crate) fn turns(&self) -> u32 {
        self.turns
    }

    /// Reads a store path after the run, through a fresh access.
    pub(crate) fn read(&self, path: &str) -> Result<String, VfsError> {
        self.store.read(path)
    }
}

/// The test store: a VFS handle, a fresh default one (a memory store at
/// `/`) unless the test supplies its own, whose `read` and `glob` helpers
/// each go through a fresh, immediately dropped access and its store view.
/// A short-lived access per call is what keeps seeding and post-run
/// assertions conflict-free: each access is its own scope, and its claims
/// die with it, so a held seeder access would meet the run's own scope as
/// a false race.
pub(crate) struct TestStore(Mutex<VfsRef>);

impl TestStore {
    pub(crate) fn new() -> TestStore {
        TestStore(Mutex::new(VfsRef::default()))
    }

    /// Wraps a caller-built handle - a gated backend, say - in the test
    /// store's seeding and post-run assertion helpers.
    pub(crate) fn from_vfs(vfs: VfsRef) -> TestStore {
        TestStore(Mutex::new(vfs))
    }

    /// The handle the run and the context builders take: the run's whole
    /// filesystem, store included.
    pub(crate) fn vfs(&self) -> VfsRef {
        self.0
            .lock()
            .expect("the store lock is not poisoned")
            .clone()
    }

    pub(crate) fn read(&self, path: &str) -> Result<String, VfsError> {
        let access = self.vfs().acquire(Origin::new("TestStore::read"))?;
        runvil_vfs::detail::store_view(&access)?.read_string(path)
    }

    pub(crate) fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        let access = self.vfs().acquire(Origin::new("TestStore::glob"))?;
        runvil_vfs::detail::store_view(&access)?.glob(pattern)
    }
}

/// `events` as the suites' observer sees them, in order.
fn records_of(events: &[Event]) -> Vec<Record> {
    let recorder = RecordObserver::default();
    forward(events.to_vec(), &recorder);
    recorder
        .0
        .into_inner()
        .expect("the record lock is not poisoned")
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
struct RecordObserver(Mutex<Vec<Record>>);

impl Observer for RecordObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the record lock is not poisoned")
            .push(Record::new(execution, section, &event.to_string()));
    }
}

/// One debug capture: which execution and section the model turn ran in,
/// its 1-based turn, and the body captured.
#[derive(Clone, Debug)]
pub(crate) struct Capture {
    pub(crate) execution: String,
    pub(crate) section: String,
    pub(crate) turn: u32,
    pub(crate) event: DebugEvent,
}

/// The debug capture [`Trace::captures`] replays the events onto.
#[derive(Default)]
struct Captures(Mutex<Vec<Capture>>);

impl DebugCapture for Captures {
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent) {
        self.0
            .lock()
            .expect("the capture lock is not poisoned")
            .push(Capture {
                execution: execution.to_owned(),
                section: section.to_owned(),
                turn: turn_index,
                event,
            });
    }
}
