//! Tests for model-task delivery: a finished model task's notice is
//! drained into the owner's message list ahead of its next round (and
//! reported as `TaskNotice` when it is queued); `await_tasks` parks the
//! model on its live tasks and resumes with the drained notices when one
//! ends, with the still-running list when its timeout fires, with
//! `nothing to wait for` when it has no task and no timeout, and as a
//! plain sleep when only a timeout is given; a sibling chain keeps
//! stepping while the model is parked; the notice text names how a
//! task ended (cancelled by the author, abandoned, failed); and a notice
//! queued after the owner left its section - at the walk's end, or between
//! two sections - reports under the section the owner last entered. A
//! scripted mock gateway plays the model, except in the between-sections
//! case, which needs the serial driver's fixed answer order. A child that
//! must still be running when the model reads it parks on the delayed
//! tool, whose calls the rig holds until the test releases them. The
//! `await_tasks` timer cases (a pending notice answers without a wait; a
//! member's end cancels the unfired timer) are in `model_task_awaits`,
//! which shares the helpers here.
//! The notice texts, the late-notice sections, and the model-issued
//! cancel sit in `endings`.

use promptforge_parser::test_support::prompt;
use promptforge_types::ids::TaskId;

use super::context::task;
use super::model_tasks::{CHILD_PARKED_ON_SLOW_TOOL, model_task_scenario};
use super::*;
use crate::test_support::recording::TaskNoticeReport;
use crate::test_support::rig::Rig;

/// The tool a child parks on until the test releases it: `tools/delayed`,
/// called with the child's name as `from`, answering `typed`.
#[track_caller]
fn delayed_tool() -> FnTool {
    FnTool::new("tools/delayed", json!({ "type": "object" }), |_| {
        Ok(ToolOutput::trusted("typed"))
    })
    .description("Answer once the test releases the call.")
}

/// Whether an effect is a call to the delayed tool, which a scenario
/// holds.
pub(super) fn calls_delayed_tool(effect: &Effect) -> bool {
    matches!(effect, Effect::ToolCall { tool, .. } if tool.to_string() == "tools/delayed")
}

/// Whether an effect is the delayed-tool call the child `from` made, for
/// a test to release.
pub(super) fn delayed_call_from(from: &'static str) -> impl Fn(&Effect) -> bool {
    move |effect| {
        calls_delayed_tool(effect)
            && matches!(effect, Effect::ToolCall { args, .. } if args["from"] == from)
    }
}

/// [`model_task_scenario`] with the delayed tool in the catalog and its
/// calls held in place of the slow tool's.
pub(super) fn delayed_task_scenario(source: impl Into<String>) -> Scenario {
    model_task_scenario(source)
        .tool(delayed_tool())
        .hold(calls_delayed_tool)
}

/// A recorder that keeps the typed observations and every `TaskNotice`
/// content report (the owner's section, the task, the text the model
/// reads), each in order.
#[derive(Default)]
pub(super) struct NoticeRecorder {
    events: Mutex<Vec<(String, Observation)>>,
    notices: Mutex<Vec<(String, TaskId, String)>>,
}

impl Observer for NoticeRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.events
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((section.to_owned(), event));
    }

    fn on_task_notice(&self, report: TaskNoticeReport<'_>) {
        self.notices
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((
                report.section.to_owned(),
                report.task.clone(),
                report.text.to_owned(),
            ));
    }
}

impl NoticeRecorder {
    /// A recorder holding what `trace`'s events report, parse events
    /// first.
    pub(super) fn replayed(trace: &Trace) -> NoticeRecorder {
        let recorder = NoticeRecorder::default();
        trace.replay(&recorder);
        recorder
    }

    pub(super) fn events(&self) -> Vec<(String, Observation)> {
        self.events
            .lock()
            .expect("the recorder mutex is not poisoned")
            .clone()
    }

    pub(super) fn notices(&self) -> Vec<(String, TaskId, String)> {
        self.notices
            .lock()
            .expect("the recorder mutex is not poisoned")
            .clone()
    }

    /// The position of the `nth` (0-based) record matching `section` and
    /// `matches`, or a panic naming what was recorded.
    fn position(&self, section: &str, nth: usize, matches: impl Fn(&Observation) -> bool) -> usize {
        let events = self.events();
        events
            .iter()
            .enumerate()
            .filter(|(_, (seen, event))| seen == section && matches(event))
            .nth(nth)
            .map_or_else(
                || panic!("no matching record #{nth} under {section} in {events:?}"),
                |(position, _)| position,
            )
    }
}

/// The owner body every test here runs: opt in to model tasks, run the
/// loop over one user message, then evaluate `tail` over `msgs`.
pub(super) fn loop_owner(tail: &str) -> String {
    format!(
        "tools.allow_tasks()\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         {tail}"
    )
}

#[test]
fn a_notice_arrives_in_the_round_after_the_task_ends() {
    // Round 1 starts the task; the child runs and ends after round 2's
    // dispatch took the notice queue, so the notice misses round 2 and
    // lands in round 3 as one user record.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task_status", "{\"id\":\"0.0\"}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            (
                "Only",
                &loop_owner("return msgs[6].role .. '|' .. msgs[6].content"),
            ),
            ("Child", "return 'child result'"),
        ],
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    let out = trace.text();
    assert!(
        out.starts_with("user|Task id=0.0 (## Child) completed: "),
        "the notice is a user record naming the task, its target, and its end: {out}"
    );
    assert!(
        out.contains("<untrusted_input_") && out.contains("child result"),
        "the task's final text reaches the model nonce-wrapped: {out}"
    );
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 3);
    let round_2 = &bodies[1].messages;
    assert_eq!(
        round_2.len(),
        3,
        "round 2 was issued before the notice existed: {round_2:?}"
    );
    let round_3 = &bodies[2].messages;
    assert_eq!(round_3.len(), 6, "round 3 holds the notice: {round_3:?}");
    assert_eq!(round_3[5].role(), Role::User);
    let notices = NoticeRecorder::replayed(&trace).notices();
    assert_eq!(notices.len(), 1, "one notice is reported: {notices:?}");
    assert_eq!(notices[0].0, "Only", "the notice reports under the owner");
    assert_eq!(notices[0].1, task("0.0"));
    assert!(
        notices[0]
            .2
            .starts_with("Task id=0.0 (## Child) completed: "),
        "the report is the text the model reads: {}",
        notices[0].2
    );
}

#[test]
fn await_tasks_returns_the_drained_notice_when_the_task_ends() {
    // The child parks on the delayed tool until the model is parked in
    // `await_tasks`; releasing the call ends the child and wakes the wait.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "await_tasks", "{}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            ("Only", &loop_owner("return msgs[5].content")),
            (
                "Child",
                "tools.call('tools/delayed', { from = 'Child' })\nreturn 'child result'",
            ),
        ],
    );
    let mut rig = Rig::start(delayed_task_scenario(md).chat(&gateway));
    rig.settle();
    rig.release(delayed_call_from("Child"));
    let rig = rig.finish();
    let out = rig.trace().text();
    assert!(
        out.starts_with("Task id=0.0 (## Child) completed: ") && out.contains("child result"),
        "await_tasks answers with the finished task's notice: {out}"
    );
    let round_3 = gateway.requests()[2].messages.len();
    assert_eq!(
        round_3, 5,
        "the notice was consumed by the wait, not drained again into round 3"
    );
    let events = NoticeRecorder::replayed(rig.trace()).events();
    assert_eq!(
        events
            .iter()
            .filter(|(section, event)| section == "Only" && *event == Observation::ToolCallSucceeded)
            .count(),
        2,
        "the start and the wait are each one succeeded call: {events:?}"
    );
}

#[test]
fn await_tasks_times_out_naming_the_tasks_still_running() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_3", "await_tasks", "{\"timeout\":0.1}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            ("Only", &loop_owner("return msgs[7].content")),
            ("Child", CHILD_PARKED_ON_SLOW_TOOL),
        ],
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "timed out; tasks 0.0, 0.1 still running",
        "the timeout resumes with content and the owner's end abandons both"
    );
}

#[test]
fn await_tasks_with_nothing_live_answers_at_once_or_sleeps() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "await_tasks", "{}")]),
        calls(&[("call_2", "await_tasks", "{\"timeout\":0.05}")]),
        calls(&[("call_3", "await_tasks", "{\"timeout\":\"soon\"}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            (
                "Only",
                &loop_owner(
                    "return msgs[3].content .. '|' .. msgs[5].content .. '|' .. msgs[7].content",
                ),
            ),
            ("Child", "return 'x'"),
        ],
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "nothing to wait for|slept 0.05 seconds|\
         await_tasks: `timeout` must be a non-negative number of seconds when given",
        "every answer is content"
    );
}

#[test]
fn a_sibling_chain_steps_while_the_model_is_parked_in_await_tasks() {
    // The author's `Sibling` task and the model's `Child` each park on a
    // delayed-tool call, and the model parks in `await_tasks`. Releasing
    // the sibling's call first lets it log while the model is parked, so
    // the sibling's log lands after the round that answered `await_tasks`
    // and before the child's end, which releasing the child's call brings.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "await_tasks", "{}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            (
                "Only",
                "tools.allow_tasks()\n\
                 local s = tasks.spawn('## Sibling')\n\
                 local msgs = messages.new()\n\
                 msgs:user('go')\n\
                 models.loop(msgs)\n\
                 local results = tasks.join({ s })\n\
                 return results[1].result .. '|' .. msgs[5].content",
            ),
            (
                "Child",
                "tools.call('tools/delayed', { from = 'Child' })\nreturn 'child result'",
            ),
            (
                "Sibling",
                "tools.call('tools/delayed', { from = 'Sibling' })\nlog('sibling ran')\nreturn 'sib'",
            ),
        ],
    );
    let mut rig = Rig::start(delayed_task_scenario(md).chat(&gateway));
    rig.settle();
    rig.release(delayed_call_from("Sibling"));
    rig.settle();
    rig.release(delayed_call_from("Child"));
    let rig = rig.finish();
    let out = rig.trace().text();
    assert!(
        out.starts_with("sib|Task id=0.1 (## Child) completed: "),
        "the sibling's result and the wait's notice both arrive: {out}"
    );
    let recorder = NoticeRecorder::replayed(rig.trace());
    let awaited = recorder.position("Only", 1, |event| *event == Observation::ModelTurnCompleted);
    let sibling_ran = recorder.position("Sibling", 0, |event| {
        *event == Observation::Lua("sibling ran".to_owned())
    });
    let child_ended = recorder.position("Child", 0, |event| {
        *event == Observation::TaskSucceeded { task: task("0.1") }
    });
    assert!(
        awaited < sibling_ran && sibling_ran < child_ended,
        "the sibling stepped while the model was parked: round 2 at {awaited}, sibling at \
         {sibling_ran}, child end at {child_ended}: {:?}",
        recorder.events()
    );
}

#[path = "model_task_notices-endings.rs"]
mod endings;
