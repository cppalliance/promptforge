//! Tests for the model's task built-ins: `tools.allow_tasks` advertises
//! `task`, `task_cancel`, and `task_status` to the model and records the
//! allowlist on the section; the `tool_call` arm answers the three by name
//! before alias lookup, refusing a target outside the allowlist; a model
//! task the owner outlives is abandoned (never cancelled) with a reason
//! naming how the owner ended; and the author's `tasks.pending` filter
//! tells the model's tasks from the author's. A scripted mock gateway plays
//! the model.

use promptforge_api_types::ids::{AbandonReason, TaskId, TaskOrigin};

use super::models_loop::loop_models;
use super::tasks::TaskRecorder;
use super::*;
use crate::execute::scheduler::{Scheduler, TaskState};
use crate::input::{InputBroker, InputError, InputOutcome};
use crate::lua::ToolSet;

/// A broker that never answers, so a child parked on `user_input()` stays
/// live until its owner ends or cancels it.
pub(super) struct NeverBroker;

#[async_trait::async_trait]
impl InputBroker for NeverBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        std::future::pending().await
    }
}

/// A child body that parks on operator input the never-answering broker
/// never gives, so the task stays live until something ends it.
pub(super) const PARKED_CHILD: &str = "user_input()\nreturn 'never'";

pub(super) fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// The run context for a model-task test: the parsed prompt, the shared
/// model set pre-filled, no bound tools, the recorder as observer, and the
/// never-answering broker so a parked child stays parked.
pub(super) fn model_task_context(prompt: &Prompt, recorder: &Arc<TaskRecorder>) -> RunState {
    model_task_context_with(
        prompt,
        Arc::clone(recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    )
}

/// [`model_task_context`] under a caller-chosen observer and input broker,
/// for the suites that time a parked child's release or record content
/// reports.
pub(super) fn model_task_context_with(
    prompt: &Prompt,
    observer: Arc<dyn Observer>,
    broker: Arc<dyn InputBroker>,
) -> RunState {
    let config = RunContext::new(EXECUTION)
        .observer(observer)
        .input_broker(broker);
    let ctx = RunState::new(
        prompt,
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &config,
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    *ctx.tool_set()
        .lock()
        .expect("the tool set mutex is not poisoned") = ToolSet::default();
    ctx
}

/// A two-section prompt: `Only` runs the model loop under `frontmatter`
/// extras, `Child` is the task target.
pub(super) fn owner_prompt(frontmatter: &str, owner_body: &str, child_body: &str) -> String {
    format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n{frontmatter}---\n\n\
         # ModelTasks\n\n\
         ## Only\n\n\
         ```lua\n{owner_body}\n```\n\n\
         ## Child\n\n\
         ```lua\n{child_body}\n```\n"
    )
}

/// The names the gateway saw advertised on `body`.
fn advertised(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "current_thread")]
async fn a_scripted_model_starts_a_task_and_reads_its_status() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("done"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks({ '## Child' })\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         assert(msgs[3].content == 'Task id=0.0 started', 'the start text: ' .. msgs[3].content)\n\
         assert(msgs[3].tool_call_id == 'call_1', 'the start correlates its call')\n\
         return msgs[5].content",
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the model starts and inspects its task");
    assert!(
        out.starts_with("Task id=0.0 (## Child): done, ok"),
        "the status text names the task, its target, and its end: {out}"
    );
    let bodies = gateway.requests();
    assert_eq!(
        advertised(&bodies[0]),
        vec!["task", "task_cancel", "task_status", "await_tasks"],
        "allow_tasks advertises exactly the answered built-ins: {bodies:?}"
    );
    let records = recorder.records();
    assert!(
        records.iter().any(|(section, event)| section == "Only"
            && matches!(
                event,
                Observation::TaskStarted {
                    task,
                    origin: TaskOrigin::Model,
                    target,
                    ..
                } if *task == self::task("0.0") && target == "Child"
            )),
        "the start is reported under the owner with the model origin: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_target_outside_the_allowlist_is_refused_naming_the_allowed_targets() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Only\"}"),
        resp_text("done"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks({ '## Child' })\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return msgs[3].content",
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the refusal is the call's content, not a raise");
    assert!(
        out.contains("## Only") && out.contains("not allowed") && out.contains("## Child"),
        "the refusal names the target and the allowlist: {out}"
    );
    let records = recorder.records();
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskStarted { .. })),
        "a refused target starts nothing: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_owner_that_ends_first_leaves_a_model_task_abandoned_not_cancelled() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         local mine = tasks.pending({ origin = 'author' })\n\
         local models = tasks.pending({ origin = 'model' })\n\
         assert(#mine == 0, 'the author started nothing')\n\
         assert(#models == 1 and models[1].task == '0.0', 'the model task is pending')\n\
         return 'ok'",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("a live model task never fails its owner as tasks_live");
    assert_eq!(out, "ok");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the model task ended with its owner as abandoned"
    );
    let records = recorder.records();
    assert!(
        records.iter().any(|(section, event)| section == "Child"
            && *event
                == Observation::TaskAbandoned {
                    task: task("0.0"),
                    reason: AbandonReason::OwnerReturned,
                }),
        "the abandonment names the owner's normal end: {records:?}"
    );
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskCancelled { .. })),
        "an abandoned task is never reported cancelled: {records:?}"
    );
    assert_eq!(AbandonReason::OwnerReturned.why(), "the section ended");
}

#[tokio::test(flavor = "current_thread")]
async fn an_exhausted_tool_loop_abandons_the_model_task_for_that_reason() {
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "task",
        "{\"target\":\"## Child\"}",
    )])
    .await;
    let md = owner_prompt(
        "max_tool_iterations: 1\n",
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return 'unreached'",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let error = scheduler
        .drive()
        .await
        .expect_err("one round then the cap fails the owner");
    assert!(
        matches!(error, Error::ToolLoopExhausted),
        "the owner's own error stands: {error:?}"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned)
    );
    let records = recorder.records();
    assert!(
        records.iter().any(|(section, event)| section == "Child"
            && *event
                == Observation::TaskAbandoned {
                    task: task("0.0"),
                    reason: AbandonReason::ToolLoopExhausted,
                }),
        "the abandonment names the exhausted loop: {records:?}"
    );
    assert_eq!(
        AbandonReason::ToolLoopExhausted.why(),
        "the tool loop was exhausted"
    );
    assert_eq!(AbandonReason::OwnerFailed.why(), "the owner failed");
}

#[tokio::test(flavor = "current_thread")]
async fn task_cancel_ends_a_model_task_and_reports_it_cancelled() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_cancel", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         assert(#tasks.pending() == 0, 'nothing is live after the cancel')\n\
         return msgs[5].content",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("the cancelled task leaves nothing live");
    assert_eq!(out, "Task id=0.0 cancelled");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Cancelled)
    );
    let records = recorder.records();
    assert!(
        records.iter().any(|(section, event)| section == "Child"
            && *event == Observation::TaskCancelled { task: task("0.0") }),
        "the cancel is reported once under the target: {records:?}"
    );
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskAbandoned { .. })),
        "a cancelled task is never also abandoned: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_model_sees_only_its_own_tasks() {
    // The author's task is `0.0`; the model's status read of it is refused
    // and the author's cancel then ends it, so the run completes clean.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         local t = tasks.spawn('## Child')\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         tasks.cancel(t)\n\
         return msgs[3].content",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the author's cancel ends its task before the chain ends");
    assert!(
        out.contains("no model task with id 0.0"),
        "an author task is invisible to the model: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn without_allow_tasks_the_built_ins_are_not_advertised() {
    let gateway = ScriptedGateway::start(vec![resp_text("bye")]).await;
    let md = owner_prompt(
        "",
        "local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return 'ok'",
        "return 'x'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool-free round completes");
    let bodies = gateway.requests();
    assert!(
        advertised(&bodies[0]).is_empty(),
        "no allowlist, no built-ins: {bodies:?}"
    );
}
