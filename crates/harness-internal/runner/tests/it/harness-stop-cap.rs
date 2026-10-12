//! A stop under the real performer's default cap of 10: the stop drops
//! the calls waiting for a slot without starting them, and a waiting call
//! that survives stops starts once the stop frees a slot.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use harness_runner::Harness;
use harness_runner::recorder::MemoryRecorder;
use promptforge::model::{Completion, CompletionResult, ToolCall};
use promptforge_plugin::HostServices;
use serde_json::json;

use super::{
    PendingTimer, RunOutcome, answers_to, answers_where, calls, request, run_beside, until,
};
use crate::scripted::{Held, MODEL, Operator, ScriptedBroker, hold_host, reply, with_asker};

/// The held tool's id.
const HOLD_TOOL: &str = "harness/hold";

/// The fixture ask tool's id, under the name the asker is installed as.
const ASK_TOOL: &str = "user-input/ask";

/// A model loop under a `pcall` that offers each of `tools` and names
/// each one's Plugin, reporting how the loop call ended.
fn loops_offering(tools: &[&str]) -> String {
    let mut plugins = String::new();
    let mut offers = String::new();
    for tool in tools {
        let plugin = tool.split('/').next().unwrap_or_default();
        writeln!(plugins, "  - {plugin}").unwrap();
        writeln!(offers, "tools.always_offer('{tool}')").unwrap();
    }
    format!(
        "---\nname: loops\ndescription: d\npromptforge: 0\nplugins:\n{plugins}\
         models:\n  writer: {{}}\n---\n\n\
         # Loops\n\n```lua\nmodels.default('writer')\n{offers}```\n\n\
         ## Only\n\n```lua\nlocal msgs = messages.new()\nmsgs:user('hold')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. ':' .. err.kind\n```\n"
    )
}

/// A broker whose first round calls each of `wire_names` in order, as
/// `c1` onward, and whose every later round replies with text.
fn calls_once(wire_names: Vec<&'static str>) -> ScriptedBroker {
    ScriptedBroker::new(move |round, _messages| {
        let wire_names = wire_names.clone();
        Box::pin(async move {
            if round.id.get() > 0 {
                return reply("done");
            }
            let calls = wire_names
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    ToolCall::from_parts(format!("c{}", index + 1), name, json!({}))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Completion::from_result(CompletionResult::ToolCalls(calls), MODEL).map(Box::new)
        })
    })
}

/// Yields enough for the run beside the test to settle, so a check that
/// a call has not started is not vacuous.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_stop_drops_waiting_calls_without_starting_them() {
    let held = Arc::new(Held::default());
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(calls_once(vec!["harness_hold"; 11])),
        Arc::new(PendingTimer::default()),
        Arc::new(hold_host(&held)),
        HostServices::new(),
    );
    let watched = Arc::clone(&held);

    let report = run_beside(
        harness,
        request(&loops_offering(&[HOLD_TOOL])),
        |control| async move {
            until("ten calls are in flight", || watched.started() >= 10).await;
            settle().await;
            assert_eq!(watched.started(), 10, "the eleventh call waits for a slot");
            control.stop_round();
        },
    )
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "false:cancelled".to_owned()
        },
        "the loop's pcall caught the dropped batch"
    );
    assert_eq!(held.started(), 10, "the waiting call never started");
    assert_eq!(held.dropped(), 10, "the stop tore down the ten in flight");
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(
        answers_to(&records, "ToolCall"),
        vec![json!("Dropped"); 11],
        "the stop answered every call Dropped, the waiting one included"
    );
}

#[tokio::test]
async fn a_stop_starts_a_waiting_call_that_survives_stops() {
    let held = Arc::new(Held::default());
    let (operator, answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let recorder = Arc::new(MemoryRecorder::new());
    let mut wire_names = vec!["harness_hold"; 10];
    wire_names.push("user-input_ask");
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(calls_once(wire_names)),
        Arc::new(PendingTimer::default()),
        Arc::new(with_asker(hold_host(&held))),
        operator.services(),
    );
    let watched = Arc::clone(&held);

    let report = run_beside(
        harness,
        request(&loops_offering(&[HOLD_TOOL, ASK_TOOL])),
        |control| async move {
            until("ten holds are in flight", || watched.started() >= 10).await;
            settle().await;
            assert_eq!(
                asked.load(Ordering::SeqCst),
                0,
                "the question waits behind the ten holds"
            );
            control.stop_round();
            until("the stop frees a slot for the question", || {
                asked.load(Ordering::SeqCst) == 1
            })
            .await;
            answers.send("kept".to_owned()).unwrap();
        },
    )
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "false:cancelled".to_owned()
        },
        "the batch answers with its first failure in call order"
    );
    assert_eq!(held.started(), 10);
    assert_eq!(held.dropped(), 10, "the stop tore down the ten holds");
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(
        answers_where(&records, "ToolCall", calls(HOLD_TOOL)),
        vec![json!("Dropped"); 10]
    );
    let asks = answers_where(&records, "ToolCall", calls(ASK_TOOL));
    assert_eq!(asks.len(), 1, "one question: {asks:?}");
    assert_eq!(
        asks[0]["ToolCall"]["Ok"]["text"], "kept",
        "the question survived the stop and was answered"
    );
}
