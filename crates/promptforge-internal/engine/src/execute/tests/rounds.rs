//! Round ids: the run numbers its model rounds from 0 in dispatch order,
//! a section's chat rounds and its nested `models.infer` rounds alike, and
//! every content event a round's answer reports - its thinking, its reply,
//! its tool-call batch - holds the id its `Chat` effect held.

use promptforge_parser::test_support::one_section;
use promptforge_types::event::{Event, ReplyOrigin};
use promptforge_types::ids::RoundId;

use super::fixtures::echo_tool;
use super::models_loop::loop_scenario;
use super::*;
use crate::execute::run::{Effect, EffectAnswer, Round};
use crate::model::{Completion, CompletionResult, ToolCall};
use crate::test_support::drive;
use crate::test_support::rig::Rig;
use crate::test_support::scenario::text_of;

/// Offers `echo`, infers once, then loops over one tool round and its
/// closing reply.
const INFER_THEN_LOOP: &str = "tools.offer('tools/echo')\n\
    local first = models.infer('first')\n\
    local msgs = messages.new()\n\
    msgs:user('second')\n\
    models.loop(msgs)\n\
    return first .. '|' .. msgs[#msgs].content";

/// The round numbered `id`, dispatched with `origin`.
fn round(id: u64, origin: ReplyOrigin) -> Round {
    Round {
        id: RoundId::new(id),
        origin,
    }
}

/// A round's answer from the test model: `result`, with `thought` as its
/// reasoning side channel.
fn thinking(result: CompletionResult, thought: &str) -> EffectAnswer {
    let completion = Completion::from_result(result, "test-model")
        .expect("a scripted result is accepted")
        .with_reasoning_content(thought);
    EffectAnswer::Chat(Ok(Box::new(completion)))
}

#[test]
fn round_ids_increase_in_dispatch_order_across_chat_and_nested_infer_rounds() {
    let gateway = ScriptedChat::new(vec![
        text("a"),
        calls(&[("call_1", "echo", r#"{"value":"hi"}"#)]),
        text("b"),
        text("c"),
    ]);
    let source = one_section(&format!(
        "{INFER_THEN_LOOP} .. '|' .. models.infer('third')"
    ));
    let rig = Rig::run(
        loop_scenario(source)
            .offer("echo", echo_tool())
            .chat(&gateway),
    );
    assert_eq!(rig.trace().text(), "a|b|c");
    assert_eq!(
        rig.rounds(),
        vec![
            round(0, ReplyOrigin::Infer),
            round(1, ReplyOrigin::Chat),
            round(2, ReplyOrigin::Chat),
            round(3, ReplyOrigin::Infer),
        ],
        "one counter numbers both kinds in dispatch order"
    );
}

#[test]
fn a_rounds_thinking_reply_and_tool_call_events_hold_the_id_its_chat_effect_held() {
    let run = loop_scenario(one_section(INFER_THEN_LOOP))
        .offer("echo", echo_tool())
        .into_run();
    let call = ToolCall::from_parts("call_1", "echo", json!({ "value": "hi" }))
        .expect("a scripted call is whole");
    let mut answers = vec![
        thinking(CompletionResult::Text("a".to_owned()), "infer thought"),
        thinking(CompletionResult::ToolCalls(vec![call]), "call thought"),
        thinking(CompletionResult::Text("b".to_owned()), "reply thought"),
    ]
    .into_iter();
    let mut issued = Vec::new();
    let (result, events) = drive(run, |_, effect| match effect {
        Effect::Chat { round, .. } => {
            issued.push(*round);
            answers.next().expect("the script covers every round")
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("echoed"))),
        other => panic!("only rounds and the echo call are issued: {other:?}"),
    });
    assert_eq!(text_of(result), "a|b");
    assert_eq!(
        issued,
        vec![
            round(0, ReplyOrigin::Infer),
            round(1, ReplyOrigin::Chat),
            round(2, ReplyOrigin::Chat),
        ],
        "the nested round has the infer origin and the loop's rounds the chat origin"
    );
    let content: Vec<(&str, RoundId)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Thinking { round, .. } => Some(("thinking", *round)),
            Event::AssistantReply { round, origin, .. } if *origin == ReplyOrigin::Infer => {
                Some(("infer reply", *round))
            }
            Event::AssistantReply { round, .. } => Some(("chat reply", *round)),
            Event::AssistantToolCalls { round, .. } => Some(("tool calls", *round)),
            _ => None,
        })
        .collect();
    assert_eq!(
        content,
        vec![
            ("thinking", RoundId::new(0)),
            ("infer reply", RoundId::new(0)),
            ("thinking", RoundId::new(1)),
            ("tool calls", RoundId::new(1)),
            ("thinking", RoundId::new(2)),
            ("chat reply", RoundId::new(2)),
        ],
        "each content event holds the id of the round that produced it"
    );
}
