//! The tool-call cap: at most the performer's `max_in_flight()` tool calls
//! run at once, the rest wait and start lowest effect id first, and a
//! waiting call that a withdrawal, a task cancel, or a run cancel reaches
//! is answered `Dropped` without ever starting.

use std::future::Future;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{BoxFuture, ToolPerformer};
use harness_runner::recorder::{MemoryRecorder, RecordKind, RunId, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::effect::ToolCallOrigin;
use promptforge::model::{
    Completion, CompletionError, CompletionResult, ModelDescriptor, ModelId, ThinkingMode, ToolCall,
};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolError, ToolId, ToolOutput};
use promptforge::vfs::Access;
use promptforge::{Environment, Prompt, Run, RunContext};
use serde_json::{Value, json};
use tokio::sync::{Notify, Semaphore};

use super::begun_log;
use crate::scripted::{Kept, MODEL, ScriptedBroker, reply};
use crate::support::{EXECUTION, unused};

/// How long a test waits for a run before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// The fixture tool every run here offers its model.
const TOOL: &str = "tests/cap/park";

/// [`TOOL`]'s wire name, as the model calls it.
const WIRE: &str = "tests_cap_park";

/// The frontmatter and setup every run here shares: one model role bound
/// as the default, and [`TOOL`] offered to every model round.
const PREAMBLE: &str = "---\nname: runner-cap\ndescription: caps tool calls\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Cap\n\n```lua\nmodels.default('writer')\ntools.always_offer('tests/cap/park')\n```\n\n";

/// Main spawns Child and waits on a `wait` round while Child's loop asks
/// its round, then cancels Child and lingers on a second round.
const MAIN_CANCELS: &str = "## Main\n\n```lua\n\
    local t = tasks.spawn('## Child')\n\
    models.infer('wait')\n\
    tasks.cancel(t)\n\
    models.infer('linger')\n\
    return 'cancelled'\n```\n\n";

/// Main spawns A, then B, and joins both.
const MAIN_JOINS: &str = "## Main\n\n```lua\n\
    local a = tasks.spawn('## A')\n\
    local b = tasks.spawn('## B')\n\
    tasks.join({ a, b })\n\
    return 'joined'\n```\n\n";

/// A section that runs a model loop over one user message, `user`, and
/// returns `result`.
fn looping(section: &str, user: &str, result: &str) -> String {
    format!(
        "## {section}\n\n```lua\nlocal msgs = messages.new()\nmsgs:user('{user}')\n\
         models.loop(msgs)\nreturn '{result}'\n```\n\n"
    )
}

/// A run over [`PREAMBLE`] and `sections`, with the one role bound to one
/// current model and [`TOOL`] in its catalog.
fn run_of(sections: &str) -> Run {
    let source = format!("{PREAMBLE}{sections}");
    let (prompt, _parse_events) = Prompt::parse(&source, EXECUTION);
    let prompt = Arc::new(prompt.expect("the cap fixture parses"));
    let model = ModelDescriptor::new(
        ModelId::gateway(MODEL).expect("the model id is valid"),
        "The host's current model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Never,
    );
    let tool = ToolDescriptor::new(
        ToolId::parse(TOOL).expect("the tool id is valid"),
        "Park until the test opens the gate.",
        json!({ "type": "object", "properties": { "name": { "type": "string" } } }),
    );
    let catalog = ToolCatalog::new(&[tool]).expect("the one-tool catalog is valid");
    let ctx = RunContext::new(EXECUTION, 7, Timestamp::UNIX_EPOCH).model(model);
    let (ctx, requirements) = Environment::new().tools(catalog).prepare(&prompt, ctx);
    assert!(
        requirements.is_satisfied(),
        "the bound model and the catalog satisfy the fixture: {requirements:?}"
    );
    Run::new(prompt, "", ctx)
}

/// Parks every call until the test adds a permit to `gate`, then answers
/// it, with at most `cap` calls in flight. Logs each call's `name`
/// argument as the call starts, on its first poll, and counts each
/// started call torn down before it answered.
struct ParkingTool {
    cap: NonZeroUsize,
    gate: Arc<Semaphore>,
    started: Kept<String>,
    torn_down: Arc<AtomicUsize>,
}

impl ParkingTool {
    fn with_cap(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            cap: NonZeroUsize::new(cap).expect("the cap is not zero"),
            gate: Arc::new(Semaphore::new(0)),
            started: Kept::default(),
            torn_down: Arc::default(),
        })
    }

    /// The `name` of every call started, in start order.
    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    /// Lets `calls` more parked calls answer.
    fn open(&self, calls: usize) {
        self.gate.add_permits(calls);
    }

    fn torn_down(&self) -> usize {
        self.torn_down.load(Ordering::SeqCst)
    }
}

impl ToolPerformer for ParkingTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _access: Arc<Access>,
        _origin: ToolCallOrigin,
        args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let name = args["name"].as_str().unwrap_or_default().to_owned();
        let gate = Arc::clone(&self.gate);
        let started = Arc::clone(&self.started);
        let torn_down = Arc::clone(&self.torn_down);
        Box::pin(async move {
            started.lock().unwrap().push(name);
            let mut torn_down = TornDown(Some(torn_down));
            gate.acquire()
                .await
                .expect("the gate semaphore is never closed")
                .forget();
            torn_down.0 = None;
            Ok(ToolOutput::trusted("opened"))
        })
    }

    fn survives_stop(&self, _tool: &ToolId) -> bool {
        false
    }

    fn max_in_flight(&self) -> NonZeroUsize {
        self.cap
    }
}

/// Counts a call torn down before it answered.
struct TornDown(Option<Arc<AtomicUsize>>);

impl Drop for TornDown {
    fn drop(&mut self) {
        if let Some(count) = &self.0 {
            count.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Plays each round through `script`, handed the round's last message.
fn answering(
    script: impl Fn(&str) -> BoxFuture<Result<Box<Completion>, CompletionError>> + Send + Sync + 'static,
) -> ScriptedBroker {
    ScriptedBroker::new(move |_round, messages| {
        let last = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        script(&last)
    })
}

/// A completion asking for [`TOOL`] once per entry of `names`, each
/// call's id and `name` argument the entry.
fn parks(names: &[&str]) -> Result<Box<Completion>, CompletionError> {
    let calls = names
        .iter()
        .map(|name| ToolCall::from_parts(*name, WIRE, json!({ "name": name })))
        .collect::<Result<Vec<_>, _>>()?;
    Completion::from_result(CompletionResult::ToolCalls(calls), MODEL).map(Box::new)
}

/// Yields until `condition` holds, or fails naming `what`. Every
/// performer here is in memory, so the run reaches each state within a
/// few polls.
async fn until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..10_000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{what}");
}

/// Yields enough for the run beside the test to settle, so a check that
/// a call has not started is not vacuous.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// One run's recorder, parking tool, and cancel flag, shared by the run
/// and the test that steers it.
struct Rig {
    recorder: Arc<MemoryRecorder>,
    run_id: RunId,
    tool: Arc<ParkingTool>,
    cancel: CancelHandle,
}

impl Rig {
    /// A rig whose tool runs at most `cap` calls at once.
    async fn with_cap(cap: usize) -> Self {
        let (recorder, run_id) = begun_log().await;
        Self {
            recorder,
            run_id,
            tool: ParkingTool::with_cap(cap),
            cancel: CancelHandle::new(),
        }
    }

    /// Drives `run` with `broker` and the rig's tool beside `steer`, and
    /// returns the run's outcome. Fails when the two outlast [`PATIENCE`].
    async fn drive(
        &self,
        run: Run,
        broker: ScriptedBroker,
        steer: impl Future<Output = ()>,
    ) -> RunOutcome {
        let mut performers = unused();
        performers.broker = Arc::new(broker);
        performers.tool = self.tool.clone();
        let drive = drive_run(
            run,
            performers,
            self.recorder.clone(),
            self.run_id,
            self.cancel.clone(),
        );
        let (outcome, ()) = tokio::time::timeout(PATIENCE, async { tokio::join!(drive, steer) })
            .await
            .expect("the run ends in time");
        outcome.expect("the drive reaches an outcome")
    }

    /// Whether the run has recorded a tool call whose `name` is `name`.
    fn recorded(&self, name: &str) -> bool {
        self.recorder.records(self.run_id).iter().any(|record| {
            record.kind == RecordKind::Effect && record.payload["ToolCall"]["args"]["name"] == name
        })
    }

    /// The answer payloads of the run's tool calls, in effect order.
    fn tool_call_answers(&self) -> Vec<Value> {
        let records = self.recorder.records(self.run_id);
        records
            .iter()
            .filter(|record| {
                record.kind == RecordKind::Effect && record.payload.get("ToolCall").is_some()
            })
            .map(|effect| {
                records
                    .iter()
                    .find(|record| {
                        record.kind == RecordKind::Answer && record.effect_id == effect.effect_id
                    })
                    .expect("every tool call is answered")
                    .payload
                    .clone()
            })
            .collect()
    }
}

fn completed(final_text: &str) -> RunOutcome {
    RunOutcome::Completed {
        final_text: final_text.to_owned(),
    }
}

#[tokio::test]
async fn the_cap_holds_extra_calls_until_one_lands() {
    let rig = Rig::with_cap(2).await;
    let broker = answering(|last| {
        let answer = if last == "go" {
            parks(&["c1", "c2", "c3"])
        } else {
            reply("done")
        };
        Box::pin(async move { answer })
    });

    let outcome = rig
        .drive(run_of(&looping("Only", "go", "done")), broker, async {
            until("two calls start", || rig.tool.started().len() >= 2).await;
            settle().await;
            assert_eq!(
                rig.tool.started(),
                ["c1", "c2"],
                "the third call waits for a slot"
            );
            rig.tool.open(1);
            until("the third call starts", || rig.tool.started().len() >= 3).await;
            rig.tool.open(2);
        })
        .await;

    assert_eq!(outcome, completed("done"));
    assert_eq!(rig.tool.started(), ["c1", "c2", "c3"]);
    assert_eq!(rig.tool_call_answers().len(), 3);
}

#[tokio::test]
async fn waiting_calls_start_oldest_first() {
    let rig = Rig::with_cap(1).await;
    let tool = Arc::clone(&rig.tool);
    let broker = answering(move |last| match last {
        "a" => Box::pin(async { parks(&["a1", "a2", "a3"]) }),
        "b" => {
            let tool = Arc::clone(&tool);
            Box::pin(async move {
                until("a1 starts", || tool.started().contains(&"a1".to_owned())).await;
                parks(&["b1"])
            })
        }
        _ => Box::pin(async { reply("done") }),
    });
    let sections = format!(
        "{MAIN_JOINS}{}{}",
        looping("A", "a", "a done"),
        looping("B", "b", "b done")
    );

    let outcome = rig
        .drive(run_of(&sections), broker, async {
            until("B's call is issued", || rig.recorded("b1")).await;
            settle().await;
            assert_eq!(rig.tool.started(), ["a1"], "the cap holds the rest");
            rig.tool.open(4);
        })
        .await;

    assert_eq!(outcome, completed("joined"));
    assert_eq!(
        rig.tool.started(),
        ["a1", "a2", "a3", "b1"],
        "A's waiting calls start before B's newer one"
    );
}

#[tokio::test]
async fn a_call_withdrawn_in_the_step_that_issued_it_never_starts() {
    let rig = Rig::with_cap(2).await;
    let child_answered = Arc::new(Notify::new());
    let broker = answering(move |last| {
        let child_answered = Arc::clone(&child_answered);
        match last {
            "go" => Box::pin(async move {
                let answer = parks(&["c1", "c2"]);
                child_answered.notify_one();
                answer
            }),
            "wait" => Box::pin(async move {
                child_answered.notified().await;
                reply("go")
            }),
            _ => Box::pin(async { reply("done") }),
        }
    });
    let sections = format!("{MAIN_CANCELS}{}", looping("Child", "go", "child done"));

    let outcome = rig.drive(run_of(&sections), broker, async {}).await;

    assert_eq!(outcome, completed("cancelled"));
    assert!(
        rig.tool.started().is_empty(),
        "a withdrawn call never starts: {:?}",
        rig.tool.started()
    );
    assert_eq!(
        rig.tool_call_answers(),
        [json!("Dropped"), json!("Dropped")]
    );
}

#[tokio::test]
async fn cancelling_a_task_drops_its_waiting_and_running_calls() {
    let rig = Rig::with_cap(1).await;
    let tool = Arc::clone(&rig.tool);
    let broker = answering(move |last| match last {
        "go" => Box::pin(async { parks(&["c1", "c2"]) }),
        "wait" => {
            let tool = Arc::clone(&tool);
            Box::pin(async move {
                until("one of Child's calls starts", || !tool.started().is_empty()).await;
                reply("go")
            })
        }
        _ => Box::pin(async { reply("done") }),
    });
    let sections = format!("{MAIN_CANCELS}{}", looping("Child", "go", "child done"));

    let outcome = rig.drive(run_of(&sections), broker, async {}).await;

    assert_eq!(outcome, completed("cancelled"));
    assert_eq!(rig.tool.started(), ["c1"], "the waiting call never started");
    assert_eq!(rig.tool.torn_down(), 1, "the running call was torn down");
    assert_eq!(
        rig.tool_call_answers(),
        [json!("Dropped"), json!("Dropped")]
    );
}

#[tokio::test]
async fn a_cancel_drops_waiting_calls_without_starting_them() {
    let rig = Rig::with_cap(1).await;
    let broker = answering(|last| {
        let answer = if last == "go" {
            parks(&["c1", "c2"])
        } else {
            reply("done")
        };
        Box::pin(async move { answer })
    });

    let outcome = rig
        .drive(run_of(&looping("Only", "go", "done")), broker, async {
            until("one call starts", || !rig.tool.started().is_empty()).await;
            rig.cancel.cancel();
        })
        .await;

    assert_eq!(outcome, RunOutcome::Cancelled);
    assert_eq!(rig.tool.started(), ["c1"], "the waiting call never started");
    assert_eq!(
        rig.tool_call_answers(),
        [json!("Dropped"), json!("Dropped")]
    );
}
