//! The run-level test driver. A [`Scenario`] holds a run's setup as data:
//! its source, args, run name, context edits, store, tools, and model. It
//! parses the source, prepares the run, and steps it on the calling thread,
//! answering every effect the moment it is issued, and returns a [`Trace`]
//! of what the run produced. Nothing awaits, spawns, or sleeps: a timer
//! fires at once, a tool is a closure, and the model is a script.
//!
//! The serial helpers the scheduler-level suites drive a bare [`Run`] with
//! sit beside it in `serial`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use promptforge_types::event::Event;
use promptforge_types::timestamp::Timestamp;
use promptforge_types::tools::{ToolCatalog, ToolId};
use promptforge_vfs::{Origin, VfsError, VfsRef};

use super::recording::{Observation, Observer};
use super::scripted_chat::ScriptedChat;
use super::tokio_driver::{refuse_chat, refuse_tool_call};
use super::{FnTool, TestTool, drive_step, forward};
use crate::execute::{
    AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Run, RunContext, RunError,
    RunResult,
};
use crate::lua::run_store_op;
use crate::parser::Prompt;

#[path = "scenario-serial.rs"]
mod serial;

pub(crate) use serial::{
    Batching, drive_batched, echo_chat, infer_prompt, perform_locally, text_of, text_reply,
    tool_call_reply,
};

/// The run name a test that does not choose one runs under.
pub(crate) const EXECUTION: &str = "execute-test";

/// The fixed caller inputs every test run shares: a seed and a start instant
/// a test that does not care about them never has to choose. The tests of
/// the inputs themselves (`run_inputs`) build their contexts directly.
pub(crate) const TEST_SEED: u64 = 1;
pub(crate) const TEST_STARTED_AT: Timestamp = Timestamp::from_unix_millis(1_700_000_000_000);

/// The edit a scenario applies to its run's context before the prepare.
type ContextEdit = Box<dyn FnOnce(RunContext) -> RunContext>;

/// A run's setup as data. [`run`](Scenario::run) drives it to its end;
/// [`start`](Scenario::start) hands back the run to step by hand.
#[must_use]
pub(crate) struct Scenario {
    source: String,
    args: String,
    name: String,
    context: ContextEdit,
    store: TestStore,
    tools: Vec<FnTool>,
    chat: Option<ScriptedChat>,
    prepare: bool,
}

impl Scenario {
    /// A run of `source` named [`EXECUTION`], under [`TEST_SEED`] and
    /// [`TEST_STARTED_AT`], with empty args, a fresh memory store, no tools,
    /// and no model, prepared as `run_with_fixture` prepares: an
    /// unsatisfiable prompt is refused before it starts.
    pub(crate) fn new(source: impl Into<String>) -> Scenario {
        Scenario {
            source: source.into(),
            args: String::new(),
            name: EXECUTION.to_owned(),
            context: Box::new(|ctx| ctx),
            store: TestStore::new(),
            tools: Vec::new(),
            chat: None,
            prepare: true,
        }
    }

    /// Sets the args string the run receives.
    pub(crate) fn args(mut self, args: &str) -> Scenario {
        args.clone_into(&mut self.args);
        self
    }

    /// Sets the run name, which the source is also parsed under.
    pub(crate) fn name(mut self, name: &str) -> Scenario {
        name.clone_into(&mut self.name);
        self
    }

    /// Adds `edit` to the edits applied to the run's context, after the
    /// scenario's own settings and before the prepare.
    pub(crate) fn context(
        mut self,
        edit: impl FnOnce(RunContext) -> RunContext + 'static,
    ) -> Scenario {
        let earlier = self.context;
        self.context = Box::new(move |ctx| edit(earlier(ctx)));
        self
    }

    /// Runs over the caller-built handle `vfs` in place of a fresh store.
    pub(crate) fn vfs(mut self, vfs: VfsRef) -> Scenario {
        self.store = TestStore::from_vfs(vfs);
        self
    }

    /// Starts the run over the context as given, with no prepare: no model
    /// role is bound and no requirement is checked.
    pub(crate) fn unprepared(mut self) -> Scenario {
        self.prepare = false;
        self
    }

    /// Adds `tool` to the run's catalog and answers its calls with it.
    pub(crate) fn tool(mut self, tool: FnTool) -> Scenario {
        self.tools.push(tool);
        self
    }

    /// Answers the run's model rounds from `chat`, which shares its script
    /// and its record of every round with the caller's copy.
    pub(crate) fn chat(mut self, chat: &ScriptedChat) -> Scenario {
        self.chat = Some(chat.clone());
        self
    }

    /// Parses the source, prepares the run, and starts it.
    ///
    /// # Panics
    /// When the source fails to parse, naming the error.
    #[track_caller]
    pub(crate) fn start(self) -> Driving {
        let (parsed, events) = Prompt::parse(&self.source, &self.name);
        let prompt = match parsed {
            Ok(prompt) => prompt,
            Err(error) => panic!("expected the scenario source to parse, got {error}"),
        };
        let mut ctx = RunContext::new(self.name, TEST_SEED, TEST_STARTED_AT).vfs(self.store.vfs());
        if !self.tools.is_empty() {
            let descriptors: Vec<_> = self.tools.iter().map(TestTool::descriptor).collect();
            let catalog = ToolCatalog::new(&descriptors).expect("the scenario's tool ids differ");
            ctx = ctx.tools(catalog);
        }
        let mut ctx = (self.context)(ctx);
        let mut refused = None;
        if self.prepare {
            let requirements;
            (ctx, requirements) = ctx.prepare(&prompt);
            refused = requirements.refusal().map(RunResult::Failure);
        }
        let answers = Answers {
            tools: self
                .tools
                .into_iter()
                .map(|tool| (tool.id(), tool))
                .collect(),
            chat: self.chat,
            timeout: ctx.limits.timeout(),
        };
        let run = refused
            .is_none()
            .then(|| Run::new(Arc::new(prompt), &self.args, ctx));
        Driving {
            run,
            result: refused,
            answers,
            events,
            effects: Vec::new(),
            store: self.store,
        }
    }

    /// Drives the run to its end and returns what it produced.
    ///
    /// # Panics
    /// When the source fails to parse, naming the error.
    #[track_caller]
    pub(crate) fn run(self) -> Trace {
        self.start().finish()
    }
}

/// How a scenario answers its run's effects: a store operation on the
/// effect's own access, a timer at once, a tool call through the
/// scenario's tool of that id, and a model round from its script.
struct Answers {
    tools: BTreeMap<ToolId, FnTool>,
    chat: Option<ScriptedChat>,
    timeout: Duration,
}

impl Answers {
    /// Answers `effect` at once. A tool the scenario does not hold is
    /// refused, and so is a model round when the scenario has no model.
    fn answer(&self, effect: &Effect) -> EffectAnswer {
        match effect {
            Effect::Chat {
                messages,
                tools,
                options,
                ..
            } => match &self.chat {
                Some(chat) => {
                    let (_, answer) = chat.complete_now(messages, tools, options, self.timeout);
                    EffectAnswer::Chat(answer.map(Box::new))
                }
                None => refuse_chat(),
            },
            Effect::ToolCall { tool, args, .. } => match self.tools.get(tool) {
                Some(tool) => EffectAnswer::ToolCall(tool.call_now(args.clone())),
                None => refuse_tool_call(),
            },
            Effect::Vfs { access, op } => EffectAnswer::Vfs(run_store_op(access, op.clone())),
            Effect::Timer { .. } => EffectAnswer::Timer,
        }
    }
}

/// A started scenario, stepped by hand or to its end.
pub(crate) struct Driving {
    /// The run, until it is done or when the prepare refused it.
    run: Option<Run>,
    result: Option<RunResult>,
    answers: Answers,
    /// The parse events, then every event the run reported, in order.
    events: Vec<Event>,
    effects: Vec<(EffectRecord, AnswerRecord)>,
    store: TestStore,
}

impl Driving {
    /// Steps the run once, answering every effect the step issued, and
    /// returns whether the run is still pending.
    pub(crate) fn step(&mut self) -> bool {
        let Some(run) = self.run.as_mut() else {
            return false;
        };
        let answers = &self.answers;
        let effects = &mut self.effects;
        let mut perform = |_: EffectId, effect: &Effect| answers.answer(effect);
        let mut answered = |effect: &Effect, answer: &EffectAnswer| {
            effects.push((effect.record(), answer.record()));
        };
        let Some(result) = drive_step(run, &mut perform, &mut answered, &mut self.events) else {
            return true;
        };
        self.result = Some(result);
        self.run = None;
        false
    }

    /// Steps the run to its end and returns what it produced.
    pub(crate) fn finish(mut self) -> Trace {
        while self.step() {}
        Trace {
            result: self
                .result
                .expect("a run that stopped stepping has its result"),
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

    /// Every effect the run issued, beside its answer, in issue order. An
    /// effect issued after the run decided its outcome is paired with
    /// [`AnswerRecord::Dropped`].
    pub(crate) fn effects(&self) -> &[(EffectRecord, AnswerRecord)] {
        &self.effects
    }

    /// The events as the suites' observer sees them, parse events first.
    pub(crate) fn records(&self) -> Vec<Record> {
        let recorder = Recorder::default();
        forward(self.events.clone(), &recorder);
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
        promptforge_vfs::detail::store_view(&access)?.read_string(path)
    }

    pub(crate) fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        let access = self.vfs().acquire(Origin::new("TestStore::glob"))?;
        promptforge_vfs::detail::store_view(&access)?.glob(pattern)
    }
}
