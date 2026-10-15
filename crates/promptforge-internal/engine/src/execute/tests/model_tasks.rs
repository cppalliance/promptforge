//! Tests for the model's task built-ins: `tools.allow_tasks` advertises
//! `task`, `task_cancel`, and `task_status` to the model and records the
//! allowlist on the section; the `tool_call` arm answers the three by name
//! before alias lookup, refusing a target outside the allowlist; a model
//! task the owner outlives is abandoned (never cancelled) with a reason
//! naming how the owner ended; a chain end holding one task of each origin
//! leaks the author's alone; and the author's `tasks.pending` filter
//! tells the model's tasks from the author's. A scripted mock gateway plays
//! the model.

use promptforge_parser::test_support::PromptSource;
use promptforge_types::ids::{AbandonReason, TaskId, TaskOrigin};

use super::models_loop::loop_models;
use super::tasks::TaskRecorder;
use super::*;
use crate::execute::scheduler::test_hooks::TaskState;
use crate::lua::ToolSet;
use crate::test_support::rig::Rig;

/// A child body that parks on a call to the never-completing tool, by its
/// full id, so the task stays live until something ends it.
pub(super) const CHILD_PARKED_ON_SLOW_TOOL: &str = "tools.call('test/tools/slow')\nreturn 'never'";

/// The run context and fixture for a model-task test: the parsed prompt,
/// the shared model set pre-filled, no bound tools, `observer` watching,
/// and `park`, the tool a child parks on, as the run's whole catalog, so
/// a section calls it by its full id without binding an alias.
pub(super) fn model_task_context_with(
    prompt: &Prompt,
    observer: Arc<dyn Observer>,
    park: Arc<dyn TestTool>,
) -> (RunState, RunFixture) {
    let (catalog, table) = fixture_tools(&[park]);
    let (prepared, requirements) = test_context(EXECUTION).tools(catalog).prepare(prompt);
    assert!(
        requirements.is_satisfied(),
        "the model-task prompt declares nothing the environment must supply: {requirements:?}"
    );
    let fixture = RunFixture::new().observer(observer).tools(table);
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        &prepared,
    )
    .with_tool_set(ToolSet::default());
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    (ctx, fixture)
}

/// [`model_task_context_with`] as a scenario over the slow tool: the loop
/// models filled in, nothing offered, and the slow tool as the whole
/// catalog, its calls held so a parked child stays parked until the run
/// decides.
pub(super) fn model_task_scenario(source: impl Into<String>) -> Scenario {
    Scenario::new(source)
        .models(loop_models())
        .offering(Vec::new())
        .tool(slow_tool())
        .hold(calls_slow_tool)
}

/// A two-section prompt: `Only` runs the model loop under `frontmatter`
/// extras, `Child` is the task target.
pub(super) fn owner_prompt(frontmatter: &str, owner_body: &str, child_body: &str) -> String {
    PromptSource::new()
        .name("mt")
        .title("ModelTasks")
        .front(frontmatter)
        .section("Only", owner_body)
        .section("Child", child_body)
        .build()
}

/// The names the gateway saw advertised on `body`.
fn advertised(body: &ScriptedCall) -> Vec<String> {
    body.tool_names().into_iter().map(str::to_owned).collect()
}

#[test]
fn a_scripted_model_starts_a_task_and_reads_its_status() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task_status", "{\"id\":\"0.0\"}")]),
        text("done"),
    ]);
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
    let rig = Rig::run(model_task_scenario(md).chat(&gateway));
    let out = rig.trace().text();
    assert!(
        out.starts_with("Task id=0.0 (## Child): done, ok"),
        "the model starts and inspects its task, and the status text names the task, \
         its target, and its end: {out}"
    );
    let bodies = gateway.requests();
    assert_eq!(
        advertised(&bodies[0]),
        vec!["task", "task_cancel", "task_status", "await_tasks"],
        "allow_tasks advertises exactly the answered built-ins: {bodies:?}"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
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

#[test]
fn a_target_outside_the_allowlist_is_refused_naming_the_allowed_targets() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Only\"}")]),
        text("done"),
    ]);
    let md = owner_prompt(
        "",
        "tools.allow_tasks({ '## Child' })\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return msgs[3].content",
        "return 'child result'",
    );
    let rig = Rig::run(model_task_scenario(md).chat(&gateway));
    let out = rig.trace().text();
    assert!(
        out.contains("## Only") && out.contains("not allowed") && out.contains("## Child"),
        "the refusal is the call's content, not a raise, and names the target and the \
         allowlist: {out}"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskStarted { .. })),
        "a refused target starts nothing: {records:?}"
    );
}

#[test]
fn an_owner_that_ends_first_leaves_a_model_task_abandoned_not_cancelled() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        text("bye"),
    ]);
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
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    let mut rig = Rig::run(model_task_scenario(md).chat(&gateway));
    assert_eq!(
        rig.trace().text(),
        "ok",
        "a live model task never fails its owner as tasks_live"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the model task ended with its owner as abandoned"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
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

#[test]
fn an_ending_owner_leaks_its_author_task_and_never_its_model_task() {
    // Both origins live under one owner at one chain end, so the origin
    // arm in `abandon_owned_tasks` is driven over every `TaskOrigin`
    // variant in a single settle: the author's id is the whole of
    // `tasks_live` and the model's is absent, while both slots end
    // abandoned. Two same-origin tasks could not tell the arm's two sides
    // apart.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        text("bye"),
    ]);
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         tasks.spawn('## Child')\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return 'ok'",
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    let mut rig = Rig::run(model_task_scenario(md).chat(&gateway));

    match rig.trace().error().inner() {
        Error::TasksLive { tasks } => assert_eq!(
            tasks,
            &[task("0.0")],
            "the live author task fails the owner it outlived, and only the author-origin \
             task is leaked: {tasks:?}"
        ),
        other => panic!("expected tasks_live, got {other:?}"),
    }
    for id in ["0.0", "0.1"] {
        assert_eq!(
            rig.task_state_for_test(&task(id)),
            Some(TaskState::Abandoned),
            "both origins ended with their owner as abandoned"
        );
    }
    let records = TaskRecorder::replayed(rig.trace()).records();
    // `abandon_owned_tasks` stamps `Abandoned` before the effect-backed
    // branch that skips the origin match, so the state alone would hold
    // for a slot that never reached the arm. The terminal observation is
    // emitted only past that branch, one statement above the arm itself.
    for id in ["0.0", "0.1"] {
        assert!(
            records.iter().any(|(section, event)| section == "Child"
                && *event
                    == Observation::TaskAbandoned {
                        task: task(id),
                        reason: AbandonReason::OwnerReturned,
                    }),
            "task {id} reached the origin arm: {records:?}"
        );
    }
    let origins: Vec<(TaskId, TaskOrigin)> = records
        .iter()
        .filter_map(|(_, event)| match event {
            Observation::TaskStarted { task, origin, .. } => Some((task.clone(), *origin)),
            _ => None,
        })
        .collect();
    assert_eq!(
        origins,
        vec![
            (task("0.0"), TaskOrigin::Author),
            (task("0.1"), TaskOrigin::Model),
        ],
        "the settle saw one task of each origin: {records:?}"
    );
}

#[test]
fn an_exhausted_tool_loop_abandons_the_queued_model_task() {
    let gateway = ScriptedChat::new(vec![calls(&[(
        "call_1",
        "task",
        "{\"target\":\"## Child\"}",
    )])]);
    let md = owner_prompt(
        "max_tool_iterations: 1\n",
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return 'unreached'",
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    let mut rig = Rig::run(model_task_scenario(md).chat(&gateway));
    let error = rig.trace().error().inner();
    assert!(
        matches!(error, Error::ToolLoopExhausted),
        "one round then the cap fails the owner, and the owner's own error stands: {error:?}"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned)
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
    // The owner exhausted before the child ever ran - the loop fails on
    // the round that answered the task call, with no suspension between -
    // so the task was still queued for admission: it reports no start
    // and no terminal, though its slot still reaches Abandoned.
    assert!(
        !records.iter().any(|(_, event)| {
            matches!(event, Observation::TaskStarted { task: seen, .. } | Observation::TaskAbandoned { task: seen, .. } if seen == &task("0.0"))
        }),
        "a task that never ran reports no start and no terminal: {records:?}"
    );
    assert_eq!(
        AbandonReason::ToolLoopExhausted.why(),
        "the tool loop was exhausted"
    );
    assert_eq!(AbandonReason::OwnerFailed.why(), "the owner failed");
}

#[test]
fn task_cancel_ends_a_model_task_and_reports_it_cancelled() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task_cancel", "{\"id\":\"0.0\"}")]),
        text("bye"),
    ]);
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         assert(#tasks.pending() == 0, 'nothing is live after the cancel')\n\
         return msgs[5].content",
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    let mut rig = Rig::run(model_task_scenario(md).chat(&gateway));
    assert_eq!(
        rig.trace().text(),
        "Task id=0.0 cancelled",
        "the cancelled task leaves nothing live"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Cancelled)
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
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

#[test]
fn the_model_sees_only_its_own_tasks() {
    // The author's task is `0.0`; the model's status read of it is refused
    // and the author's cancel then ends it, so the run completes clean.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task_status", "{\"id\":\"0.0\"}")]),
        text("bye"),
    ]);
    let md = owner_prompt(
        "",
        "tools.allow_tasks()\n\
         local t = tasks.spawn('## Child')\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         tasks.cancel(t)\n\
         return msgs[3].content",
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    let rig = Rig::run(model_task_scenario(md).chat(&gateway));
    let out = rig.trace().text();
    assert!(
        out.contains("no model task with id 0.0"),
        "the author's cancel ends its task before the chain ends, and an author task is \
         invisible to the model: {out}"
    );
}

#[test]
fn without_allow_tasks_the_built_ins_are_not_advertised() {
    let gateway = ScriptedChat::new(vec![text("bye")]);
    let md = owner_prompt(
        "",
        "local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return 'ok'",
        "return 'x'",
    );
    let rig = Rig::run(model_task_scenario(md).chat(&gateway));
    assert_eq!(rig.trace().text(), "ok", "a tool-free round completes");
    let bodies = gateway.requests();
    assert!(
        advertised(&bodies[0]).is_empty(),
        "no allowlist, no built-ins: {bodies:?}"
    );
}
