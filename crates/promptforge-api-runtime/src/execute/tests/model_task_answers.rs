//! Tests for the text the model's task built-ins answer with: every field
//! of a `task_status` line (a parked task with its section, wait, own
//! tasks, and note; a failed task) and the refusal for each malformed
//! argument shape `task`, `task_cancel`, and `task_status` reject. A
//! scripted mock gateway plays the model. The refusal for a `task` call
//! in a section with no allowlist has no round here: without
//! `tools.allow_tasks` the built-ins are not advertised, so the round's
//! scope gate refuses the call before the arm sees it.

use super::model_tasks::{model_task_context, owner_prompt, task};
use super::tasks::TaskRecorder;
use super::*;
use crate::execute::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn task_status_reports_a_parked_task_with_its_section_wait_tasks_and_note() {
    // `Child` publishes a note, spawns `Leaf`, then parks on input, so the
    // status read exercises every live-chain field of the rendering.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
              # ModelTasks\n\n\
              ## Only\n\n\
              ```lua\n\
              tools.allow_tasks({ '## Child' })\n\
              local msgs = messages.new()\n\
              msgs:user('go')\n\
              models.loop(msgs)\n\
              return msgs[5].content\n\
              ```\n\n\
              ## Child\n\n\
              ```lua\n\
              tasks.note('halfway')\n\
              tasks.spawn('## Leaf')\n\
              user_input()\n\
              return 'never'\n\
              ```\n\n\
              ## Leaf\n\n\
              ```lua\n\
              user_input()\n\
              return 'never'\n\
              ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the owner's end abandons the parked task and its leaf");
    assert_eq!(
        out,
        "Task id=0.0 (## Child): running, in ## Child, waiting on user_input, turns 0, \
         tasks 0.0.0, note: halfway",
        "a live task reports where it is, what it waits on, its tasks, and its note"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn task_status_reports_a_failed_task_as_done_failed() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks({ '## Child' })\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         return msgs[5].content",
        "error('boom')",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a failed model task never fails its owner");
    assert_eq!(
        out, "Task id=0.0 (## Child): done, failed, turns 0",
        "a finished task reports its outcome and no live-chain fields"
    );
    let records = recorder.records();
    assert!(
        records.iter().any(|(section, event)| section == "Child"
            && *event == Observation::TaskFailed { task: task("0.0") }),
        "the failure is reported under the target: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_built_in_arguments_are_refused_with_the_engine_text() {
    // One round per refusal: `task` without a target, with a non-string
    // target, with a non-string input; `task_status` and `task_cancel`
    // without an id, with a non-string id, with an unparsable id. Each
    // refusal is the call's content, and none starts a task.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{}"),
        resp_tool_call("call_2", "task", "{\"target\":7}"),
        resp_tool_call("call_3", "task", "{\"target\":\"## Child\",\"input\":7}"),
        resp_tool_call("call_4", "task_status", "{}"),
        resp_tool_call("call_5", "task_cancel", "{\"id\":5}"),
        resp_tool_call("call_6", "task_status", "{\"id\":\"nope\"}"),
        resp_text("done"),
    ])
    .await;
    let md = owner_prompt(
        "",
        "tools.allow_tasks({ '## Child' })\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         local answers = {}\n\
         for i = 3, 13, 2 do answers[#answers + 1] = msgs[i].content end\n\
         return table.concat(answers, '\\n')",
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = model_task_context(&prompt, &recorder);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("every refusal is content, not a raise");
    let target = "task: `target` must be a string naming a section heading, such as `## Research`";
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        vec![
            target,
            target,
            "task: `input` must be a string when given",
            "task_status: `id` must be a task id string, exactly as `task` returned it",
            "task_cancel: `id` must be a task id string, exactly as `task` returned it",
            "task_status: `nope` is not a task id; use the id `task` returned",
        ],
        "each refusal names the argument and what it must be"
    );
    let records = recorder.records();
    assert_eq!(
        records
            .iter()
            .filter(|(section, event)| section == "Only" && *event == Observation::ToolCallFailed)
            .count(),
        6,
        "every refusal is observed as a failed tool call: {records:?}"
    );
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskStarted { .. })),
        "a refused call starts nothing: {records:?}"
    );
}
