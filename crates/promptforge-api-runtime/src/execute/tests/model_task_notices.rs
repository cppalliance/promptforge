//! Tests for model-task delivery: a finished model task's notice is
//! drained into the owner's message list ahead of its next round (and
//! reported as `TaskNotice` when it is queued); `await_tasks` parks the
//! model on its live tasks and resumes with the drained notices when one
//! ends, with the still-running list when its timeout fires, with
//! `nothing to wait for` when it has no task and no timeout, and as a
//! plain sleep when only a timeout is given; a sibling chain keeps
//! stepping while the model is parked; and the notice text names how a
//! task ended (cancelled by the author, abandoned, failed). A scripted
//! mock gateway plays the model. The `await_tasks` timer cases (a pending
//! notice answers without a wait; a member's end cancels the unfired
//! timer) are in `model_task_awaits`, which shares the helpers here.

use std::collections::VecDeque;
use std::time::Duration;

use promptforge_api_types::ids::TaskId;

use super::model_tasks::{NeverBroker, PARKED_CHILD, model_task_context_with, owner_prompt, task};
use super::*;
use crate::input::{InputError, InputOutcome};
use crate::test_support::TestBroker;
use crate::test_support::tokio_driver::TokioDriver;

/// A broker that answers each `user_input` in call order after the next
/// scripted delay, so a parked child's release is timed by the test.
pub(super) struct DelayedBroker(Mutex<VecDeque<Duration>>);

impl DelayedBroker {
    pub(super) fn new(delays: &[Duration]) -> Arc<Self> {
        Arc::new(Self(Mutex::new(delays.iter().copied().collect())))
    }
}

#[async_trait::async_trait]
impl TestBroker for DelayedBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        let delay = self
            .0
            .lock()
            .expect("the delay queue mutex is not poisoned")
            .pop_front()
            .unwrap_or_default();
        tokio::time::sleep(delay).await;
        Ok(InputOutcome::Text("typed".to_owned()))
    }
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

    fn on_task_notice(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        _turn: u32,
        task: &TaskId,
        text: &str,
    ) {
        self.notices
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((section.to_owned(), task.clone(), text.to_owned()));
    }
}

impl NoticeRecorder {
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

#[tokio::test(flavor = "current_thread")]
async fn a_notice_arrives_in_the_round_after_the_task_ends() {
    // Round 1 starts the task; the child runs and ends while the owner is
    // between its drain and its round-2 chat, so the notice misses round 2
    // and lands in round 3 as one user record.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner("return msgs[6].role .. '|' .. msgs[6].content"),
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the notice is a message, not a raise");
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
    let round_2 = bodies[1]["messages"].as_array().expect("messages");
    assert_eq!(
        round_2.len(),
        3,
        "round 2 was issued before the notice existed: {round_2:?}"
    );
    let round_3 = bodies[2]["messages"].as_array().expect("messages");
    assert_eq!(round_3.len(), 6, "round 3 holds the notice: {round_3:?}");
    assert_eq!(round_3[5]["role"], "user");
    let notices = recorder.notices();
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

#[tokio::test(flavor = "current_thread")]
async fn await_tasks_returns_the_drained_notice_when_the_task_ends() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "await_tasks", "{}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner("return msgs[5].content"),
        "user_input()\nreturn 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&[Duration::from_millis(300)]),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the wait resumes with content");
    assert!(
        out.starts_with("Task id=0.0 (## Child) completed: ") && out.contains("child result"),
        "await_tasks answers with the finished task's notice: {out}"
    );
    let round_3 = gateway.requests()[2]["messages"]
        .as_array()
        .expect("messages")
        .len();
    assert_eq!(
        round_3, 5,
        "the notice was consumed by the wait, not drained again into round 3"
    );
    let events = recorder.events();
    assert_eq!(
        events
            .iter()
            .filter(|(section, event)| section == "Only" && *event == Observation::ToolCallSucceeded)
            .count(),
        2,
        "the start and the wait are each one succeeded call: {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn await_tasks_times_out_naming_the_tasks_still_running() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_3", "await_tasks", "{\"timeout\":0.1}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt("", &loop_owner("return msgs[7].content"), PARKED_CHILD);
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the timeout resumes with content and the owner's end abandons both");
    assert_eq!(out, "timed out; tasks 0.0, 0.1 still running");
}

#[tokio::test(flavor = "current_thread")]
async fn await_tasks_with_nothing_live_answers_at_once_or_sleeps() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "await_tasks", "{}"),
        resp_tool_call("call_2", "await_tasks", "{\"timeout\":0.05}"),
        resp_tool_call("call_3", "await_tasks", "{\"timeout\":\"soon\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner("return msgs[3].content .. '|' .. msgs[5].content .. '|' .. msgs[7].content"),
        "return 'x'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("every answer is content");
    assert_eq!(
        out,
        "nothing to wait for|slept 0.05 seconds|\
         await_tasks: `timeout` must be a non-negative number of seconds when given"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_sibling_chain_steps_while_the_model_is_parked_in_await_tasks() {
    // The author's `Sibling` task parks on input answered at 300ms; the
    // model's `Child` on input answered at 900ms. The model parks in
    // `await_tasks` within a few ms, so the sibling's log lands after the
    // round that answered `await_tasks` and before the child's end.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "await_tasks", "{}"),
        resp_text("bye"),
    ])
    .await;
    let md = "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
              # ModelTasks\n\n\
              ## Only\n\n\
              ```lua\n\
              tools.allow_tasks()\n\
              local s = tasks.spawn('## Sibling')\n\
              local msgs = messages.new()\n\
              msgs:user('go')\n\
              models.loop(msgs)\n\
              local results = tasks.when_all({ s })\n\
              return results[1].result .. '|' .. msgs[5].content\n\
              ```\n\n\
              ## Child\n\n\
              ```lua\nuser_input()\nreturn 'child result'\n```\n\n\
              ## Sibling\n\n\
              ```lua\nuser_input()\nlog('sibling ran')\nreturn 'sib'\n```\n";
    let prompt = parse(md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&[Duration::from_millis(300), Duration::from_millis(900)]),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("both tasks end and the owner collects them");
    assert!(
        out.starts_with("sib|Task id=0.1 (## Child) completed: "),
        "the sibling's result and the wait's notice both arrive: {out}"
    );
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

/// Runs the two-section prompt with the model starting `Child` in round 1
/// and replying in round 2, then returns every notice reported.
async fn notices_for(owner_tail: &str, child_body: &str) -> Vec<(String, TaskId, String)> {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt("", &loop_owner(owner_tail), child_body);
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the owner ends clean");
    recorder.notices()
}

#[tokio::test(flavor = "current_thread")]
async fn notice_texts_name_how_a_task_ended() {
    let cancelled = notices_for(
        "local mine = tasks.pending({ origin = 'model' })\n\
         tasks.cancel(mine[1])\n\
         return 'ok'",
        PARKED_CHILD,
    )
    .await;
    assert_eq!(
        cancelled,
        vec![(
            "Only".to_owned(),
            task("0.0"),
            "Task id=0.0 (## Child) was canceled: the author cancelled it".to_owned()
        )],
        "an author cancel of a model task is one notice"
    );

    let abandoned = notices_for("return 'ok'", PARKED_CHILD).await;
    assert_eq!(
        abandoned,
        vec![(
            "Only".to_owned(),
            task("0.0"),
            "Task id=0.0 (## Child) was abandoned: the section ended".to_owned()
        )],
        "an owner ending first is one abandonment notice"
    );

    let failed = notices_for("return 'ok'", "error('boom')").await;
    assert_eq!(failed.len(), 1, "a failed task is one notice: {failed:?}");
    assert!(
        failed[0].2.starts_with("Task id=0.0 (## Child) failed: ") && failed[0].2.contains("boom"),
        "the failure notice reports the task's error: {}",
        failed[0].2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_model_issued_cancel_queues_no_notice() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_cancel", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt("", &loop_owner("return 'ok'"), PARKED_CHILD);
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the cancel leaves nothing live");
    assert!(
        recorder.notices().is_empty(),
        "the model already read `Task id=0.0 cancelled`; no notice repeats it: {:?}",
        recorder.notices()
    );
}
