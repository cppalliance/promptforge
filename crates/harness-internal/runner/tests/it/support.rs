//! Fixtures shared by the runner's suites: a one-section prompt, a run
//! over it, and fake performers that answer by script.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use harness_log::{RunId, RunOutcome};
use harness_runner::effect_loop::SharedLog;
use harness_runner::performers::{
    BoxFuture, ChatPerformer, Performers, StorePerformer, TaskEventsPerformer, TimerPerformer,
    ToolPerformer,
};
use promptforge::event::Event;
use promptforge::ids::TaskId;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ToolSchema,
};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolError, ToolId, ToolOutput};
use promptforge::vfs::Access;
use promptforge::vfs::{StoreOp, StoreOutcome, VfsError};
use promptforge::{Environment, Prompt, Run, RunContext};
use serde_json::{Value, json};

/// The run's execution identifier.
pub(crate) const EXECUTION: &str = "runner-test";

/// A section body that calls the fixture wait tool by its full id and
/// returns its answer: the section parks until the test's tool performer
/// answers.
pub(crate) const WAITS: &str = "return tools.call('tests/runner/wait')";

/// The catalog every fixture run is prepared against: the one wait tool
/// [`WAITS`] calls, so a section reaches it by full id without binding an
/// alias.
fn catalog() -> ToolCatalog {
    let wait = ToolDescriptor::new(
        ToolId::parse("tests/runner/wait").expect("the wait tool id is valid"),
        "wait",
        "Wait until the test's tool performer answers.",
        json!({ "type": "object", "properties": {} }),
    );
    ToolCatalog::new(&[wait]).expect("the one-tool catalog is valid")
}

/// A prompt whose one section runs `lua` as its only block.
pub(crate) fn prompt(lua: &str) -> Arc<Prompt> {
    let source = format!(
        "---\nname: runner-test\ndescription: a runner fixture\npromptforge: 0\n---\n\n\
         # Fixture\n\n## Only\n\n```lua\n{lua}\n```\n"
    );
    let (prompt, _parse_events) = Prompt::parse(&source, EXECUTION);
    Arc::new(prompt.expect("the fixture prompt parses"))
}

/// A capability-free run over `prompt` with a fixed seed and start,
/// prepared against the fixture [`catalog`].
fn prepared(prompt: Arc<Prompt>) -> Run {
    let ctx = RunContext::new(EXECUTION, 7, Timestamp::UNIX_EPOCH);
    let (ctx, requirements) = Environment::new().tools(catalog()).prepare(&prompt, ctx);
    assert!(
        requirements.is_satisfied(),
        "the runner fixture prompt declares nothing the host must supply: {requirements:?}"
    );
    Run::new(prompt, "", ctx)
}

/// A capability-free run over `lua` with a fixed seed and start.
pub(crate) fn run(lua: &str) -> Run {
    prepared(prompt(lua))
}

/// A capability-free run over two sections: `## Main` runs `main`, and
/// `## Child` runs `child` when the main spawns it as a task.
pub(crate) fn run_with_child(main: &str, child: &str) -> Run {
    let source = format!(
        "---\nname: runner-test\ndescription: a runner fixture\npromptforge: 0\n---\n\n\
         # Fixture\n\n## Main\n\n```lua\n{main}\n```\n\n## Child\n\n```lua\n{child}\n```\n"
    );
    let (prompt, _parse_events) = Prompt::parse(&source, EXECUTION);
    let prompt = Arc::new(prompt.expect("the two-section fixture prompt parses"));
    prepared(prompt)
}

/// A main section that parks on a 30-second timer beside its child: the
/// child's own tool call and the timer are two effects out at once.
pub(crate) const TIMED_MAIN: &str = "local t = tasks.spawn('## Child')\n\
     local _first, _ok, result = tasks.join_any({ t }, { timeout = 30 })\n\
     return result";

/// A performer for every kind that no test here expects to be issued;
/// reaching one is the test's failure.
pub(crate) struct Unused;

impl ChatPerformer for Unused {
    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _stream: bool,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        unreachable!("no test issues a Chat effect")
    }
}

impl ToolPerformer for Unused {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        unreachable!("this test issues no ToolCall effect")
    }
}

impl StorePerformer for Unused {
    fn perform(&self, _access: &Access, _op: StoreOp) -> Result<StoreOutcome, VfsError> {
        unreachable!("this test issues no Store effect")
    }
}

impl TimerPerformer for Unused {
    fn sleep(&self, _seconds: f64) -> BoxFuture<()> {
        unreachable!("no test issues a Timer effect")
    }
}

impl TaskEventsPerformer for Unused {
    fn events(&self, _task: TaskId, _last: Option<u32>) -> BoxFuture<Vec<Event>> {
        unreachable!("no test issues a TaskEvents effect")
    }
}

/// The bundle with every slot unused; a test overrides the kinds it
/// issues.
pub(crate) fn unused() -> Performers {
    let unused = Arc::new(Unused);
    Performers {
        chat: unused.clone(),
        tool: unused.clone(),
        store: unused.clone(),
        timer: unused.clone(),
        task_events: unused,
    }
}

/// Answers every tool call with the same trusted text.
pub(crate) struct TextTool(pub(crate) &'static str);

impl ToolPerformer for TextTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let text = self.0;
        Box::pin(async move { Ok(ToolOutput::trusted(text)) })
    }
}

/// Stays pending forever: the tool call that never returns.
pub(crate) struct PendingTool;

impl ToolPerformer for PendingTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        Box::pin(std::future::pending())
    }
}

/// Panics instead of answering: a performer the Harness lost to a bug. The
/// call panics on its first poll.
pub(crate) struct PanickingTool;

impl ToolPerformer for PanickingTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        Box::pin(std::future::poll_fn(
            |_cx| -> Poll<Result<ToolOutput, ToolError>> {
                panic!("the tool performer panics instead of answering")
            },
        ))
    }
}

/// Closes the run's row in the log before answering, so the loop's next
/// write is refused: the log failing under a live run.
pub(crate) struct ClosingTool {
    pub(crate) log: SharedLog,
    pub(crate) run_id: RunId,
}

impl ToolPerformer for ClosingTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let log = Arc::clone(&self.log);
        let run_id = self.run_id;
        Box::pin(async move {
            log.lock()
                .await
                .end_run(run_id, RunOutcome::Cancelled)
                .await
                .expect("the open row closes");
            Ok(ToolOutput::trusted("late"))
        })
    }
}

/// Raises its flag when dropped: how a test sees a future torn down.
struct RaiseOnDrop(Arc<AtomicBool>);

impl Drop for RaiseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Never fires, and raises `dropped` when its sleep is torn down: the
/// timer a cancel or an abort must reach.
pub(crate) struct PendingTimer {
    pub(crate) dropped: Arc<AtomicBool>,
}

impl TimerPerformer for PendingTimer {
    fn sleep(&self, _seconds: f64) -> BoxFuture<()> {
        let raise = RaiseOnDrop(Arc::clone(&self.dropped));
        Box::pin(async move {
            let _raise = raise;
            std::future::pending::<()>().await;
        })
    }
}

/// Answers every store operation with the unit outcome at once.
pub(crate) struct UnitStore;

impl StorePerformer for UnitStore {
    fn perform(&self, _access: &Access, _op: StoreOp) -> Result<StoreOutcome, VfsError> {
        Ok(StoreOutcome::Unit)
    }
}

/// Blocks for `delay` before answering, and raises `finished` when it has.
pub(crate) struct SlowStore {
    pub(crate) delay: Duration,
    pub(crate) finished: Arc<AtomicBool>,
}

impl StorePerformer for SlowStore {
    fn perform(&self, _access: &Access, _op: StoreOp) -> Result<StoreOutcome, VfsError> {
        std::thread::sleep(self.delay);
        self.finished.store(true, Ordering::SeqCst);
        Ok(StoreOutcome::Unit)
    }
}
