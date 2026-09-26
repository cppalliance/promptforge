//! Tests for the turn a model-issued tool result reports: every result in
//! a batch carries the turn of the round that requested the batch, even
//! when an earlier local handler in that batch runs model rounds of its
//! own - through `models.infer`, its own `models.loop`, or a `call` into a
//! section that runs one - and advances the chain's turn counter before
//! the later calls dispatch. Bound tools, local tools, and the task
//! built-ins (answered at once, parked in `await_tasks`, or issued as a
//! `task_events` read) all report the requesting round's turn.

use super::models_loop::{echo_tools, loop_context_observed, loop_prompt};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;
use promptforge_types::event::ReplyOrigin;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

/// One turn-bearing report: a requested batch (its call ids joined by
/// commas), a tool result (its call id), or an assistant reply (its text).
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnReport {
    Batch { turn: u32, ids: String },
    Result { turn: u32, id: String },
    Reply { turn: u32, text: String },
}

/// Records the turn on every batch, result, and reply, in order.
#[derive(Default)]
struct TurnRecorder(Mutex<Vec<TurnReport>>);

impl TurnRecorder {
    fn push(&self, report: TurnReport) {
        self.0
            .lock()
            .expect("the turn recorder mutex is not poisoned")
            .push(report);
    }

    fn reports(&self) -> Vec<TurnReport> {
        self.0
            .lock()
            .expect("the turn recorder mutex is not poisoned")
            .clone()
    }

    /// The turn of the batch whose ids are exactly `ids`.
    fn batch_turn(&self, ids: &str) -> u32 {
        let reports = self.reports();
        reports
            .iter()
            .find_map(|report| match report {
                TurnReport::Batch { turn, ids: seen } if seen == ids => Some(*turn),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no batch {ids} in {reports:?}"))
    }

    /// The turn of the one result reported under `id`.
    fn result_turn(&self, id: &str) -> u32 {
        let reports = self.reports();
        let turns: Vec<u32> = reports
            .iter()
            .filter_map(|report| match report {
                TurnReport::Result { turn, id: seen } if seen == id => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns.len(), 1, "one result under {id}: {reports:?}");
        turns[0]
    }

    /// The turn of the reply whose text is `text`.
    fn reply_turn(&self, text: &str) -> u32 {
        let reports = self.reports();
        reports
            .iter()
            .find_map(|report| match report {
                TurnReport::Reply { turn, text: seen } if seen == text => Some(*turn),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no reply {text:?} in {reports:?}"))
    }
}

impl Observer for TurnRecorder {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}

    fn on_assistant_reply(
        &self,
        _execution: &str,
        _section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        text: &str,
        _finish_reason: Option<&str>,
        _model: &str,
        _metrics: Option<&CallMetrics>,
        _origin: ReplyOrigin,
    ) {
        self.push(TurnReport::Reply {
            turn,
            text: text.to_owned(),
        });
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        _section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        _model: &str,
        calls: &[ToolCallEvent],
    ) {
        let ids: Vec<&str> = calls.iter().map(|call| call.id.as_str()).collect();
        self.push(TurnReport::Batch {
            turn,
            ids: ids.join(","),
        });
    }

    fn on_tool_result(
        &self,
        _execution: &str,
        _section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        tool_call_id: &str,
        _alias: &str,
        _content: &str,
        _trusted: bool,
    ) {
        self.push(TurnReport::Result {
            turn,
            id: tool_call_id.to_owned(),
        });
    }
}

/// A response requesting every `(id, name, arguments)` call in one batch.
fn resp_batch(calls: &[(&str, &str, &str)]) -> GatewayReply {
    let tool_calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, arguments)| {
            json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": arguments }
            })
        })
        .collect();
    GatewayReply::Json(json!({
        "model": MOCK_MODEL,
        "choices": [{
            "message": { "role": "assistant", "content": null, "tool_calls": tool_calls }
        }]
    }))
}

/// The block every test here runs: registers `grab` with `handler` as its
/// body, then one loop over a single user message, returning the final
/// record's text. `setup` runs first.
fn grab_block(setup: &str, handler: &str) -> String {
    format!(
        "{setup}\
         tools.add_local('grab', 'Grab a value', {{ value = 'string' }}, function(args)\n\
           {handler}\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tools.')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content"
    )
}

/// The `grab` handler body that runs one inference round.
const INFER_HANDLER: &str = "return 'inferred ' .. models.infer('inner prompt')";

/// Drives `md` against `replies` under `tools`, returning the run's output
/// and the recorder.
async fn drive(
    md: &str,
    tools: impl Into<FixtureTools>,
    replies: Vec<GatewayReply>,
) -> (String, Arc<TurnRecorder>) {
    let gateway = ScriptedGateway::start(replies).await;
    let prompt = parse(md);
    let recorder = Arc::new(TurnRecorder::default());
    let (ctx, host) =
        loop_context_observed(&prompt, tools, Arc::clone(&recorder) as Arc<dyn Observer>);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the batch runs to the final reply");
    (out, recorder)
}

#[tokio::test(flavor = "current_thread")]
async fn a_later_bound_result_reports_the_batch_turn_after_a_handler_infers() {
    let (out, recorder) = drive(
        &loop_prompt(&grab_block("", INFER_HANDLER)),
        echo_tools(),
        vec![
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            resp_text("inner reply"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let batch = recorder.batch_turn("c1,c2");
    assert_eq!(
        recorder.reply_turn("inner reply"),
        batch + 1,
        "the handler's inference advanced the counter mid-batch: {:?}",
        recorder.reports()
    );
    assert_eq!(
        recorder.result_turn("c1"),
        batch,
        "the handler's own result"
    );
    assert_eq!(
        recorder.result_turn("c2"),
        batch,
        "the bound call dispatched after the handler reports the batch turn: {:?}",
        recorder.reports()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_handlers_own_loop_reports_its_round_and_the_outer_batch_keeps_its_turn() {
    let handler = "local inner = messages.new()\n\
                   inner:user('inner')\n\
                   models.loop(inner)\n\
                   return inner[#inner].content";
    let (out, recorder) = drive(
        &loop_prompt(&grab_block("", handler)),
        echo_tools(),
        vec![
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            resp_tool_call("i1", "echo", "{\"value\":\"x\"}"),
            resp_text("inner done"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let outer = recorder.batch_turn("c1,c2");
    let inner = recorder.batch_turn("i1");
    assert!(inner > outer, "the inner round follows the outer one");
    assert_eq!(
        recorder.result_turn("i1"),
        inner,
        "the inner result reports the inner round"
    );
    assert_eq!(recorder.result_turn("c1"), outer);
    assert_eq!(
        recorder.result_turn("c2"),
        outer,
        "the outer batch's later call reports the outer turn: {:?}",
        recorder.reports()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_later_result_keeps_the_batch_turn_after_a_handler_calls_an_inferring_section() {
    let md = format!(
        "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n## Only\n\n```lua\n{}\n```\n\n\
         ## Helper\n\n```lua\nreturn models.infer('helper prompt')\n```\n",
        grab_block("", "return call('## Helper')")
    );
    let (out, recorder) = drive(
        &md,
        echo_tools(),
        vec![
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            resp_text("helper reply"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let batch = recorder.batch_turn("c1,c2");
    assert_eq!(
        recorder.reply_turn("helper reply"),
        batch + 1,
        "the called section shares its caller's counter: {:?}",
        recorder.reports()
    );
    assert_eq!(
        recorder.result_turn("c2"),
        batch,
        "the call dispatched after the handler reports the batch turn: {:?}",
        recorder.reports()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_later_task_status_answer_reports_the_batch_turn() {
    let (out, recorder) = drive(
        &loop_prompt(&grab_block("tools.allow_tasks()\n", INFER_HANDLER)),
        ToolSet::default(),
        vec![
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "task_status", "{\"id\":\"0.0\"}"),
            ]),
            resp_text("inner reply"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let batch = recorder.batch_turn("c1,c2");
    assert_eq!(recorder.reply_turn("inner reply"), batch + 1);
    assert_eq!(
        recorder.result_turn("c2"),
        batch,
        "the built-in's answer reports the batch turn: {:?}",
        recorder.reports()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_parked_await_tasks_answer_reports_the_turn_it_was_dispatched_under() {
    let (out, recorder) = drive(
        &loop_prompt(&grab_block("tools.allow_tasks()\n", INFER_HANDLER)),
        ToolSet::default(),
        vec![
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "await_tasks", "{\"timeout\":0.01}"),
            ]),
            resp_text("inner reply"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let batch = recorder.batch_turn("c1,c2");
    assert_eq!(recorder.reply_turn("inner reply"), batch + 1);
    assert_eq!(
        recorder.result_turn("c2"),
        batch,
        "the wake reports the turn stored at dispatch: {:?}",
        recorder.reports()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_task_events_answer_reports_the_turn_it_was_dispatched_under() {
    let md = format!(
        "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n## Only\n\n```lua\n{}\n```\n\n\
         ## Child\n\n```lua\nreturn 'child result'\n```\n",
        grab_block("tools.allow_tasks()\n", INFER_HANDLER)
    );
    let (out, recorder) = drive(
        &md,
        ToolSet::default(),
        vec![
            resp_tool_call("c0", "task", "{\"target\":\"## Child\"}"),
            resp_batch(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "task_events", "{\"id\":\"0.0\"}"),
            ]),
            resp_text("inner reply"),
            resp_text("final answer"),
        ],
    )
    .await;
    assert_eq!(out, "final answer");
    let batch = recorder.batch_turn("c1,c2");
    assert_eq!(recorder.reply_turn("inner reply"), batch + 1);
    assert_eq!(
        recorder.result_turn("c2"),
        batch,
        "the history read's answer reports the turn stored at dispatch: {:?}",
        recorder.reports()
    );
}
