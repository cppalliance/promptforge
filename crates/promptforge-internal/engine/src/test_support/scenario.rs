//! The run-level test driver. A [`Scenario`] holds a run's setup as data:
//! its source, args, run name, seed, context edits, store, tools, model,
//! the sets it fills in before the run starts, the effects it holds, and
//! the effect that cancels it. It parses the source, prepares the run, and
//! steps it on the calling thread, answering every effect it does not hold
//! the moment it is issued, and returns a [`Trace`] of what the run
//! produced. Nothing awaits or sleeps: a timer fires at once, a tool is a
//! closure, and the model is a script or a closure. Only a canceller runs
//! on a second thread, and the scenario joins it.
//!
//! The serial helpers the scheduler-level suites drive a bare [`Run`] with
//! sit beside it in `serial`, and the started run, how it answers its
//! effects, its store, and its trace in `driving`.

use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use promptforge_model_client::model::ModelInvocation;
use promptforge_types::event::Event;
use promptforge_types::seed::RunSeed;
use promptforge_types::timestamp::Timestamp;
use promptforge_types::tools::ToolCatalog;
use promptforge_vfs::VfsRef;

use super::FnTool;
use super::scripted_chat::ScriptedChat;
use crate::Error;
use crate::execute::{Effect, EffectAnswer, Run, RunContext, RunError, RunLimits, RunResult};
use crate::lua::{ToolBinding, ToolSet};
use crate::model::{
    CompletionErrorKind, ModelBinding, ModelDescriptor, ModelId, ModelSet, ThinkingMode,
};
use crate::parser::Prompt;

#[path = "scenario-driving.rs"]
mod driving;
#[path = "scenario-serial.rs"]
mod serial;

use driving::Answers;
pub(crate) use driving::{Driving, Record, TestStore, Trace};
pub(crate) use serial::{
    Batching, chat_answer, drive_batched, echo_chat, infer_prompt, only_effect, round_trip, text_of,
};

/// The run name a test that does not choose one runs under.
pub(crate) const EXECUTION: &str = "execute-test";

/// The fixed caller inputs every test run shares: a seed and a start instant
/// a test that does not care about them never has to choose. The tests of
/// the inputs themselves (`run_inputs`) build their contexts directly.
pub(crate) const TEST_SEED: RunSeed = RunSeed::from_bytes([1; 32]);
pub(crate) const TEST_STARTED_AT: Timestamp = Timestamp::from_unix_millis(1_700_000_000_000);

/// The gateway model a bound scenario's roles resolve to.
pub(crate) const TEST_MODEL: &str = "claude-sonnet-4-6";

/// The gateway model `id` as the tests bind it: a 131072-token window and
/// switchable thinking.
#[track_caller]
pub(crate) fn test_model(id: &str) -> ModelDescriptor {
    test_model_with(id, 131_072, ThinkingMode::Switchable)
}

/// The gateway model `id` on a `window`-token context window, with
/// `thinking`.
#[track_caller]
pub(crate) fn test_model_with(id: &str, window: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway(id).expect("the test model id is valid"),
        "A general model for tests",
        NonZeroU32::new(window).expect("the test model's window is non-zero"),
        thinking,
    )
}

/// The binding `alias` to the gateway model `model` as a test fills a
/// model set by hand: described as `description`, on a 4096-token window,
/// and with no request settings.
#[track_caller]
pub(crate) fn test_binding(alias: &str, description: &str, model: &str) -> ModelBinding {
    ModelBinding::new(
        alias,
        description,
        ModelId::gateway(model).expect("the test model id is valid"),
        ModelInvocation::new(),
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

/// Asserts `error` is a model failure of `kind` whose message is `message`.
#[track_caller]
pub(crate) fn assert_model_failure(error: &RunError, kind: CompletionErrorKind, message: &str) {
    let Error::Completion(failure) = error.inner() else {
        panic!("expected a model failure, got {error:?}");
    };
    assert_eq!(
        (failure.kind(), failure.to_string().as_str()),
        (kind, message)
    );
}

/// The edit a scenario applies to its run's context before the prepare.
type ContextEdit = Box<dyn FnOnce(RunContext) -> RunContext>;

/// A test over an effect: which effects a scenario holds, or which answer
/// signals its canceller.
type Matcher = Box<dyn Fn(&Effect) -> bool>;

/// How a scenario answers its run's model rounds.
enum Chat {
    /// From a script, under the run's timeout.
    Script(ScriptedChat),
    /// Through a closure handed each round's effect.
    Closure(Box<dyn Fn(&Effect) -> EffectAnswer>),
}

/// A run's setup as data. [`run`](Scenario::run) drives it to its end;
/// [`start`](Scenario::start) hands back the run to step by hand.
#[must_use]
pub(crate) struct Scenario {
    source: String,
    args: String,
    name: String,
    seed: RunSeed,
    context: ContextEdit,
    store: TestStore,
    /// The catalog's tools.
    tools: Vec<FnTool>,
    /// The tools behind offered bindings, outside the catalog.
    offered: Vec<FnTool>,
    prefill: Prefill,
    chat: Option<Chat>,
    hold: Option<Matcher>,
    signal: Option<Matcher>,
    prepare: bool,
    bind_roles: bool,
}

impl Scenario {
    /// A run of `source` named [`EXECUTION`], under [`TEST_SEED`] and
    /// [`TEST_STARTED_AT`], with empty args, a fresh memory store, no tools,
    /// no model, nothing filled in, and no held effect, prepared so that
    /// an unsatisfiable prompt is refused before it starts.
    pub(crate) fn new(source: impl Into<String>) -> Scenario {
        Scenario {
            source: source.into(),
            args: String::new(),
            name: EXECUTION.to_owned(),
            seed: TEST_SEED,
            context: Box::new(|ctx| ctx),
            store: TestStore::new(),
            tools: Vec::new(),
            offered: Vec::new(),
            prefill: Prefill::default(),
            chat: None,
            hold: None,
            signal: None,
            prepare: true,
            bind_roles: false,
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

    /// Sets the seed the run's untrusted-envelope nonce derives from.
    pub(crate) fn seed(mut self, seed: RunSeed) -> Scenario {
        self.seed = seed;
        self
    }

    /// Makes the [`TEST_MODEL`] the context's current model, so the
    /// prepare's trivial fill binds every role the prompt declares to it.
    pub(crate) fn bound(self) -> Scenario {
        self.context(|ctx| ctx.model(test_model(TEST_MODEL)))
    }

    /// Binds every role the prompt declares to the [`TEST_MODEL`] in the
    /// context's model bindings, as the prepare's fill would, without making
    /// it the current model, so the run's model set starts empty for the
    /// live H1 pass to fill.
    pub(crate) fn bind_roles(mut self) -> Scenario {
        self.bind_roles = true;
        self
    }

    /// Runs under `limits` in place of the default limits.
    pub(crate) fn limits(self, limits: RunLimits) -> Scenario {
        self.context(move |ctx| ctx.limits(limits))
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

    /// Fills the run's model set with `set` before the run starts, as the
    /// live H1 pass would leave it, in place of the set the prepare binds.
    pub(crate) fn models(mut self, set: ModelSet) -> Scenario {
        self.prefill.models = Some(set);
        self
    }

    /// Replaces the run's offering with `bindings` before the run starts,
    /// in place of the one the catalog builds. A call to a binding with no
    /// scenario tool behind it is refused.
    pub(crate) fn offering(mut self, bindings: Vec<ToolBinding>) -> Scenario {
        self.prefill.offering = Some(bindings);
        self
    }

    /// Adds `tool` to the run's offering under the wire name `alias` and
    /// answers its calls with it. The tool stays out of the catalog, and,
    /// as with [`offering`](Scenario::offering), the offered tools replace
    /// the offering the catalog builds.
    pub(crate) fn offer(mut self, alias: &str, tool: FnTool) -> Scenario {
        let binding = ToolBinding::from_descriptor(alias, &tool.descriptor());
        self.prefill
            .offering
            .get_or_insert_with(Vec::new)
            .push(binding);
        self.offered.push(tool);
        self
    }

    /// Exposes the raw `tools.call_as_model` shim in every section VM, so
    /// a section can issue a model-issued tool call itself.
    pub(crate) fn raw_shims(mut self) -> Scenario {
        self.prefill.raw_shims = true;
        self
    }

    /// Answers the run's model rounds from `chat`, which shares its script
    /// and its record of every round with the caller's copy.
    pub(crate) fn chat(mut self, chat: &ScriptedChat) -> Scenario {
        self.chat = Some(Chat::Script(chat.clone()));
        self
    }

    /// Answers the run's model rounds through `answer`, handed each round's
    /// effect, for a model a script cannot play.
    pub(crate) fn chat_with(
        mut self,
        answer: impl Fn(&Effect) -> EffectAnswer + 'static,
    ) -> Scenario {
        self.chat = Some(Chat::Closure(Box::new(answer)));
        self
    }

    /// Leaves every effect `held` matches unanswered while the run keeps
    /// stepping, so a cancel can land between steps. Once the run decides
    /// its outcome, each held effect is answered `Dropped`.
    pub(crate) fn hold(mut self, held: impl Fn(&Effect) -> bool + 'static) -> Scenario {
        self.hold = Some(Box::new(held));
        self
    }

    /// Cancels the run from a second thread once the scenario answers or
    /// holds an effect `signal` matches. The thread waits for that effect
    /// untimed, so the cancel lands at some step after the signal, not
    /// necessarily the next. The stepper joins the thread once a step
    /// answers nothing and holds nothing new, so a run left waiting only on
    /// held effects always sees the cancel before it moves again. Use it
    /// only where the outcome is the same whichever step the cancel lands
    /// in. Either way, the scenario joins the thread before it hands back
    /// the trace.
    pub(crate) fn cancel_when(mut self, signal: impl Fn(&Effect) -> bool + 'static) -> Scenario {
        self.signal = Some(Box::new(signal));
        self
    }

    /// Parses the source, prepares the run, and starts it.
    ///
    /// # Panics
    /// When the source fails to parse, naming the error.
    #[track_caller]
    pub(crate) fn start(mut self) -> Driving {
        let (prompt, events, ctx, refused) = self.prepared();
        let (signal, signalled) = match self.signal {
            Some(signal) => {
                let (sender, receiver) = mpsc::channel();
                (Some((signal, sender)), Some(receiver))
            }
            None => (None, None),
        };
        let answers = Answers {
            tools: (self.tools.into_iter().chain(self.offered))
                .map(|tool| (tool.id(), tool))
                .collect(),
            chat: self.chat,
            hold: self.hold,
            signal,
            signalled: Cell::new(false),
            timeout: ctx.limits.timeout(),
        };
        let run = refused
            .is_none()
            .then(|| self.prefill.start(Arc::new(prompt), &self.args, ctx));
        let canceller = run.as_ref().zip(signalled).map(|(run, signalled)| {
            let cancel = run.cancel_handle();
            thread::spawn(move || {
                if signalled.recv().is_ok() {
                    cancel.cancel();
                }
            })
        });
        Driving {
            run,
            result: refused,
            answers,
            parsed: events.len(),
            events,
            effects: Vec::new(),
            held: Vec::new(),
            store: self.store,
            canceller,
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

    /// Parses the source, prepares the run, and hands it back unstarted,
    /// for a test that steps it and answers its effects by hand. The
    /// scenario's tools still shape the catalog and the offering, but its
    /// answers, holds, and canceller play no part.
    ///
    /// # Panics
    /// When the source fails to parse or the prepare refuses the run,
    /// naming the error.
    #[track_caller]
    pub(crate) fn into_run(mut self) -> Run {
        let (prompt, _, ctx, refused) = self.prepared();
        if let Some(refused) = refused {
            panic!("expected the prepare to accept the run, got {refused:?}");
        }
        self.prefill.start(Arc::new(prompt), &self.args, ctx)
    }

    /// Parses the source and builds the run's context, prepared unless the
    /// scenario skips the prepare, beside the parse events and the
    /// prepare's refusal.
    #[track_caller]
    fn prepared(&mut self) -> (Prompt, Vec<Event>, RunContext, Option<RunResult>) {
        let (parsed, events) = Prompt::parse(&self.source, &self.name);
        let prompt = match parsed {
            Ok(prompt) => prompt,
            Err(error) => panic!("expected the scenario source to parse, got {error}"),
        };
        let mut ctx =
            RunContext::new(self.name.clone(), self.seed, TEST_STARTED_AT).vfs(self.store.vfs());
        if !self.tools.is_empty() {
            let catalog = ToolCatalog::new(self.tools.iter().map(FnTool::descriptor))
                .expect("the scenario's tool ids differ");
            ctx = ctx.tools(catalog);
        }
        let edit = std::mem::replace(&mut self.context, Box::new(|ctx| ctx));
        let mut ctx = edit(ctx);
        if self.bind_roles {
            for (label, _) in prompt.frontmatter().models().iter() {
                ctx.bind_role_for_test(label, test_model(TEST_MODEL));
            }
        }
        let mut refused = None;
        if self.prepare {
            let requirements;
            (ctx, requirements) = ctx.prepare(&prompt);
            refused = requirements.refusal().map(RunResult::Failure);
        }
        (prompt, events, ctx, refused)
    }
}

/// What a scenario fills into its run's state before the run starts, in
/// place of what the prepare, the live H1 pass, and the offering would
/// leave there.
#[derive(Default)]
struct Prefill {
    models: Option<ModelSet>,
    offering: Option<Vec<ToolBinding>>,
    raw_shims: bool,
}

impl Prefill {
    /// Builds the run of `prompt` under `ctx` with the state filled in, or
    /// as [`Run::new`] builds it when there is nothing to fill.
    fn start(self, prompt: Arc<Prompt>, args: &str, ctx: RunContext) -> Run {
        if self.models.is_none() && self.offering.is_none() && !self.raw_shims {
            return Run::new(prompt, args, ctx);
        }
        Run::new_for_test(prompt, args, &ctx, |mut state| {
            if let Some(set) = self.models {
                state = state.with_models_for_test(set);
            }
            if let Some(offered) = self.offering {
                state = state.with_tool_set(ToolSet {
                    offered,
                    ..ToolSet::default()
                });
            }
            if self.raw_shims {
                state.expose_raw_shims_for_test();
            }
            state
        })
    }
}
