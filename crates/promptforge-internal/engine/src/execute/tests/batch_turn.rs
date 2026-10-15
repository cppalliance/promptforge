//! Tests for the turn a model-issued tool result reports: every result in
//! a batch carries the turn of the round that requested the batch, even
//! when an earlier local handler in that batch runs model rounds of its
//! own - through `models.infer`, its own `models.loop`, or a `call` into a
//! section that runs one - and advances the chain's turn counter before
//! the later calls dispatch. Bound tools, local tools, and the task
//! built-ins (answered at once or parked in `await_tasks`) all report the
//! requesting round's turn.

use promptforge_parser::test_support::{one_section, prompt};
use promptforge_types::event::Event;

use super::models_loop::loop_scenario;
use super::*;
use crate::test_support::scenario::Trace;

/// One turn-bearing report: a requested batch (its call ids joined by
/// commas), a tool result (its call id), or an assistant reply (its text).
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnReport {
    Batch { turn: u32, ids: String },
    Result { turn: u32, id: String },
    Reply { turn: u32, text: String },
}

/// The turn on every batch, result, and reply a run reported, in order.
#[derive(Debug)]
struct Turns(Vec<TurnReport>);

impl Turns {
    /// The turn-bearing reports among `trace`'s events.
    fn of(trace: &Trace) -> Turns {
        let reports = trace.events().iter().filter_map(|event| match event {
            Event::AssistantToolCalls { turn, calls, .. } => {
                let ids: Vec<&str> = calls.iter().map(|call| call.id.as_str()).collect();
                Some(TurnReport::Batch {
                    turn: *turn,
                    ids: ids.join(","),
                })
            }
            Event::ToolResult {
                turn, tool_call_id, ..
            } => Some(TurnReport::Result {
                turn: *turn,
                id: tool_call_id.clone(),
            }),
            Event::AssistantReply { turn, text, .. } => Some(TurnReport::Reply {
                turn: *turn,
                text: text.clone(),
            }),
            _ => None,
        });
        Turns(reports.collect())
    }

    /// The turn of the batch whose ids are exactly `ids`.
    #[track_caller]
    fn batch_turn(&self, ids: &str) -> u32 {
        self.0
            .iter()
            .find_map(|report| match report {
                TurnReport::Batch { turn, ids: seen } if seen == ids => Some(*turn),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no batch {ids} in {:?}", self.0))
    }

    /// The turn of the one result reported under `id`.
    #[track_caller]
    fn result_turn(&self, id: &str) -> u32 {
        let turns: Vec<u32> = self
            .0
            .iter()
            .filter_map(|report| match report {
                TurnReport::Result { turn, id: seen } if seen == id => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns.len(), 1, "one result under {id}: {:?}", self.0);
        turns[0]
    }

    /// The turn of the reply whose text is `text`.
    #[track_caller]
    fn reply_turn(&self, text: &str) -> u32 {
        self.0
            .iter()
            .find_map(|report| match report {
                TurnReport::Reply { turn, text: seen } if seen == text => Some(*turn),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no reply {text:?} in {:?}", self.0))
    }
}

/// The block every test here runs: registers `grab` with `handler` as its
/// body, then one loop over a single user message, returning the final
/// record's text. `setup` runs first.
fn grab_block(setup: &str, handler: &str) -> String {
    format!(
        "{setup}\
         tools.offer_local('grab', 'Grab a value', {{ value = 'string' }}, function(args)\n\
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

/// The [`grab_block`] setup that puts the `echo` fixture in scope.
const OFFER_ECHO: &str = "tools.offer('tools/echo')\n";

/// Drives `scenario` against `replies`, returning the run's output and its
/// turn reports.
#[track_caller]
fn drive(scenario: Scenario, replies: Vec<ScriptedReply>) -> (String, Turns) {
    let gateway = ScriptedChat::new(replies);
    let trace = scenario.chat(&gateway).run();
    (trace.text().to_owned(), Turns::of(&trace))
}

#[test]
fn a_later_bound_result_reports_the_batch_turn_after_a_handler_infers() {
    let (out, turns) = drive(
        loop_scenario(one_section(&grab_block(OFFER_ECHO, INFER_HANDLER)))
            .offer("echo", echo_tool()),
        vec![
            calls(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            text("inner reply"),
            text("final answer"),
        ],
    );
    assert_eq!(out, "final answer");
    let batch = turns.batch_turn("c1,c2");
    assert_eq!(
        turns.reply_turn("inner reply"),
        batch + 1,
        "the handler's inference advanced the counter mid-batch: {turns:?}"
    );
    assert_eq!(turns.result_turn("c1"), batch, "the handler's own result");
    assert_eq!(
        turns.result_turn("c2"),
        batch,
        "the bound call dispatched after the handler reports the batch turn: {turns:?}"
    );
}

#[test]
fn a_handlers_own_loop_reports_its_round_and_the_outer_batch_keeps_its_turn() {
    let handler = "local inner = messages.new()\n\
                   inner:user('inner')\n\
                   models.loop(inner)\n\
                   return inner[#inner].content";
    let (out, turns) = drive(
        loop_scenario(one_section(&grab_block(OFFER_ECHO, handler))).offer("echo", echo_tool()),
        vec![
            calls(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            calls(&[("i1", "echo", "{\"value\":\"x\"}")]),
            text("inner done"),
            text("final answer"),
        ],
    );
    assert_eq!(out, "final answer");
    let outer = turns.batch_turn("c1,c2");
    let inner = turns.batch_turn("i1");
    assert!(inner > outer, "the inner round follows the outer one");
    assert_eq!(
        turns.result_turn("i1"),
        inner,
        "the inner result reports the inner round"
    );
    assert_eq!(turns.result_turn("c1"), outer);
    assert_eq!(
        turns.result_turn("c2"),
        outer,
        "the outer batch's later call reports the outer turn: {turns:?}"
    );
}

#[test]
fn a_later_result_keeps_the_batch_turn_after_a_handler_calls_an_inferring_section() {
    let source = prompt(
        "",
        &[
            ("Only", &grab_block(OFFER_ECHO, "return call('## Helper')")),
            ("Helper", "return models.infer('helper prompt')"),
        ],
    );
    let (out, turns) = drive(
        loop_scenario(source).offer("echo", echo_tool()),
        vec![
            calls(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "echo", "{\"value\":\"b\"}"),
            ]),
            text("helper reply"),
            text("final answer"),
        ],
    );
    assert_eq!(out, "final answer");
    let batch = turns.batch_turn("c1,c2");
    assert_eq!(
        turns.reply_turn("helper reply"),
        batch + 1,
        "the called section shares its caller's counter: {turns:?}"
    );
    assert_eq!(
        turns.result_turn("c2"),
        batch,
        "the call dispatched after the handler reports the batch turn: {turns:?}"
    );
}

#[test]
fn a_later_task_status_answer_reports_the_batch_turn() {
    let (out, turns) = drive(
        loop_scenario(one_section(&grab_block(
            "tools.allow_tasks()\n",
            INFER_HANDLER,
        ))),
        vec![
            calls(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "task_status", "{\"id\":\"0.0\"}"),
            ]),
            text("inner reply"),
            text("final answer"),
        ],
    );
    assert_eq!(out, "final answer");
    let batch = turns.batch_turn("c1,c2");
    assert_eq!(turns.reply_turn("inner reply"), batch + 1);
    assert_eq!(
        turns.result_turn("c2"),
        batch,
        "the built-in's answer reports the batch turn: {turns:?}"
    );
}

#[test]
fn a_parked_await_tasks_answer_reports_the_turn_it_was_dispatched_under() {
    let (out, turns) = drive(
        loop_scenario(one_section(&grab_block(
            "tools.allow_tasks()\n",
            INFER_HANDLER,
        ))),
        vec![
            calls(&[
                ("c1", "grab", "{\"value\":\"a\"}"),
                ("c2", "await_tasks", "{\"timeout\":0.01}"),
            ]),
            text("inner reply"),
            text("final answer"),
        ],
    );
    assert_eq!(out, "final answer");
    let batch = turns.batch_turn("c1,c2");
    assert_eq!(turns.reply_turn("inner reply"), batch + 1);
    assert_eq!(
        turns.result_turn("c2"),
        batch,
        "the wake reports the turn stored at dispatch: {turns:?}"
    );
}
