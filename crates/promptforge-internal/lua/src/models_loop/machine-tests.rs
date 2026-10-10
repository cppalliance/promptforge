//! Tests for the `models.loop` machine, with no VM: `&str` values in
//! place of the Lua values it passes along, and a plain list read back
//! through `records()`.

use promptforge_types::metrics::ToolCallEvent;
use serde_json::json;

use super::{Input, Machine, Phase, Then};
use crate::compactors::OverflowReason;
use crate::messages::MessageList;
use crate::protocol::{ChatResult, MessageContent, MessageRecord};

type V = &'static str;

/// One step as a line: the action and the values it names.
fn show(then: &Then<V>) -> String {
    match then {
        Then::Chat { messages, handle } => format!("chat {messages} {}", handle.unwrap_or("-")),
        Then::ToolCall { call, turn } => {
            format!(
                "tool_call {} {} {}@{turn}",
                call.id, call.name, call.arguments
            )
        }
        Then::Compact { compactor, reason } => {
            format!(
                "compact {compactor} {}",
                reason.map_or("nil", OverflowReason::tag)
            )
        }
        Then::Handle { handler, args } => format!("handle {handler} {args}"),
        Then::Report(returned) => format!("report {}", returned.unwrap_or("raised")),
        Then::Return => "return".to_owned(),
        Then::Raise(value) => format!("raise {value}"),
        Then::RaiseNormalized(value) => format!("normalize {value}"),
        Then::RaiseNew(raised) => match raised.fields.get("finish_reason") {
            Some(reason) => format!("new {}|{}|{reason:?}", raised.kind, raised.message),
            None => format!("new {}|{}", raised.kind, raised.message),
        },
    }
}

/// The list's records as lines: role, text, calls, and the answered id.
fn records(list: &MessageList) -> Vec<String> {
    let line = |record: &MessageRecord| {
        let MessageContent::Text(text) = &record.content else {
            panic!("the loop adds text records only");
        };
        let calls: Vec<String> = record
            .tool_calls
            .iter()
            .map(|call| call.id.clone())
            .collect();
        let id = record.tool_call_id.as_deref().unwrap_or("");
        format!("{} {text}|{}|{id}", record.role.as_str(), calls.join(","))
    };
    list.records().iter().map(line).collect()
}

/// A requested call named `echo`, its arguments naming its id.
fn call(id: &str) -> ToolCallEvent {
    ToolCallEvent {
        id: id.to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "id": id }),
        tool: None,
    }
}

/// A served round with no product, under turn 7.
fn round() -> ChatResult {
    ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: None,
        finish_reason: None,
        model: String::new(),
        metrics: None,
        turn: 7,
    }
}

/// The `chat` answer `round`.
fn answered(round: ChatResult) -> Input<V> {
    Input::Answered(Ok(Box::new(round)))
}

/// A round requesting one call per id, in order.
fn batch(ids: &[&str]) -> Input<V> {
    answered(ChatResult {
        tool_calls: Some(ids.iter().copied().map(call).collect()),
        ..round()
    })
}

/// A round replying `text`.
fn reply(text: &str) -> Input<V> {
    answered(ChatResult {
        reply: Some(text.to_owned()),
        ..round()
    })
}

/// An empty round that finished with `stop`.
fn stop() -> Input<V> {
    answered(ChatResult {
        finish_reason: Some("stop".to_owned()),
        ..round()
    })
}

/// A bound tool's answer `text`.
fn text(text: &str) -> Input<V> {
    Input::Dispatched(Ok(text.to_owned()))
}

/// A raw `pcall`'s `outcome`, under the cancel flag `cancelled`.
fn called(outcome: Result<V, V>, cancelled: bool) -> Input<V> {
    Input::Called { outcome, cancelled }
}

/// A `models.loop` call capped at `cap` rounds over `list`, at its first
/// chat.
fn chatting(cap: usize, list: &MessageList) -> Machine<V> {
    let (machine, first) = Machine::begin(cap, list.clone(), "list", None, "compactor");
    assert_eq!(show(&first), "chat list -");
    machine
}

#[test]
fn a_zero_cap_raises_tool_loop_exhausted_before_any_chat() {
    let (machine, first) = Machine::begin(0, MessageList::default(), "list", None, "compactor");
    assert_eq!(
        show(&first),
        "new tool_loop_exhausted|tool-call loop did not converge"
    );
    assert!(matches!(machine.phase(), Phase::Done));
}

#[test]
fn a_round_starts_with_the_chat_naming_the_handle_and_pushes_nothing() {
    let list = MessageList::default();
    let (machine, first) = Machine::begin(2, list.clone(), "list", Some("h"), "compactor");
    assert_eq!(show(&first), "chat list h");
    assert!(matches!(machine.phase(), Phase::Chatting));
    assert!(
        records(&list).is_empty(),
        "the chat dispatch adds the notices, not the machine"
    );
}

#[test]
fn a_round_is_judged_overflow_then_calls_then_reply_then_clean_exit_then_empty() {
    let full = ChatResult {
        overflow: true,
        overflow_reason: Some(OverflowReason::Precheck),
        reply: Some("text".to_owned()),
        tool_calls: Some(vec![call("c1")]),
        finish_reason: Some("stop".to_owned()),
        empty_detail: Some("nothing".to_owned()),
        ..round()
    };
    let cases = [
        (full.clone(), "compact compactor precheck"),
        (
            ChatResult {
                overflow: false,
                ..full.clone()
            },
            r#"tool_call c1 echo {"id":"c1"}@7"#,
        ),
        (
            ChatResult {
                overflow: false,
                tool_calls: None,
                ..full.clone()
            },
            "return",
        ),
        (
            ChatResult {
                overflow: false,
                tool_calls: None,
                reply: Some(String::new()),
                ..full
            },
            r#"new empty_model_reply|nothing|String("stop")"#,
        ),
    ];
    for (answer, expected) in cases {
        let list = MessageList::default();
        let mut machine = chatting(3, &list);
        assert_eq!(show(&machine.step(answered(answer))), expected);
    }
}

#[test]
fn a_batch_dispatches_in_order_and_appends_its_records_only_once_complete() {
    let list = MessageList::default();
    let mut machine = chatting(3, &list);
    assert_eq!(
        show(&machine.step(batch(&["c1", "c2"]))),
        r#"tool_call c1 echo {"id":"c1"}@7"#
    );
    assert_eq!(
        show(&machine.step(text("one"))),
        r#"tool_call c2 echo {"id":"c2"}@7"#
    );
    assert!(
        records(&list).is_empty(),
        "a half-answered batch adds nothing"
    );
    assert_eq!(show(&machine.step(text("two"))), "chat list -");
    assert_eq!(show(&machine.step(reply("done"))), "return");
    assert!(matches!(machine.phase(), Phase::Done));
    assert_eq!(
        records(&list),
        [
            "assistant |c1,c2|",
            "tool one||c1",
            "tool two||c2",
            "assistant done||"
        ]
    );
}

#[test]
fn the_clean_exit_counts_answered_calls_across_batches_and_resets_per_call() {
    let list = MessageList::default();
    let mut machine = chatting(4, &list);
    for id in ["c1", "c2"] {
        machine.step(batch(&[id]));
        assert_eq!(show(&machine.step(text("ok"))), "chat list -");
    }
    assert_eq!(show(&machine.step(stop())), "return");
    assert_eq!(
        records(&list).last().map(String::as_str),
        Some("assistant ||")
    );
    let mut fresh = chatting(4, &list);
    assert_eq!(
        show(&fresh.step(stop())),
        r#"new empty_model_reply|empty model reply|String("stop")"#,
        "a new call starts its count at zero"
    );
}

#[test]
fn the_cap_raises_after_the_last_allowed_batch_and_before_another_chat() {
    let list = MessageList::default();
    let mut machine = chatting(1, &list);
    machine.step(batch(&["c1"]));
    assert_eq!(
        show(&machine.step(text("ok"))),
        "new tool_loop_exhausted|tool-call loop did not converge"
    );
    assert!(matches!(machine.phase(), Phase::Done));
    assert_eq!(records(&list), ["assistant |c1|", "tool ok||c1"]);
}

#[test]
fn an_empty_reply_raises_its_detail_or_the_fallback() {
    let detailed = ChatResult {
        empty_detail: Some("the reply was empty".to_owned()),
        finish_reason: Some("length".to_owned()),
        ..round()
    };
    let mut machine = chatting(2, &MessageList::default());
    assert_eq!(
        show(&machine.step(answered(detailed))),
        r#"new empty_model_reply|the reply was empty|String("length")"#
    );
    let mut machine = chatting(2, &MessageList::default());
    assert_eq!(
        show(&machine.step(answered(round()))),
        "new empty_model_reply|empty model reply"
    );
}

#[test]
fn a_failed_answer_raises_its_value_unchanged() {
    let mut machine = chatting(2, &MessageList::default());
    assert_eq!(
        show(&machine.step(Input::Answered(Err("chat")))),
        "raise chat"
    );
    let mut machine = chatting(2, &MessageList::default());
    machine.step(batch(&["c1"]));
    assert_eq!(
        show(&machine.step(Input::Dispatched(Err("tool")))),
        "raise tool"
    );
}

/// A machine waiting on a local call's handler, over `list`.
fn handling(list: &MessageList) -> Machine<V> {
    let mut machine = chatting(2, list);
    machine.step(batch(&["c1"]));
    let local = Input::Local {
        handler: "handler",
        args: "args",
    };
    assert_eq!(show(&machine.step(local)), "handle handler args");
    machine
}

#[test]
fn a_local_call_reports_its_outcome_then_raises_the_handlers_value_first() {
    let list = MessageList::default();
    let mut machine = handling(&list);
    assert_eq!(
        show(&machine.step(called(Ok("value"), false))),
        "report value"
    );
    assert_eq!(
        show(&machine.step(Input::Reported(Ok("text".to_owned())))),
        "chat list -"
    );
    assert_eq!(records(&list)[1], "tool text||c1");
    let mut machine = handling(&list);
    machine.step(called(Ok("value"), false));
    assert_eq!(
        show(&machine.step(Input::Reported(Err("refused")))),
        "raise refused"
    );
    for answer in [Ok("text".to_owned()), Err("refused")] {
        let mut machine = handling(&list);
        assert_eq!(
            show(&machine.step(called(Err("own"), false))),
            "report raised"
        );
        assert_eq!(show(&machine.step(Input::Reported(answer))), "raise own");
    }
}

#[test]
fn cancellation_raises_a_failed_handler_or_compactor_raw_and_a_return_still_wins() {
    let list = MessageList::default();
    let mut machine = handling(&list);
    assert_eq!(
        show(&machine.step(called(Err("abort"), true))),
        "raise abort"
    );
    let mut machine = handling(&list);
    assert_eq!(
        show(&machine.step(called(Ok("value"), true))),
        "report value"
    );
    let overflow = ChatResult {
        overflow: true,
        ..round()
    };
    let cases = [
        (Err("abort"), true, "raise abort"),
        (Err("failure"), false, "normalize failure"),
        (
            Ok("returned"),
            true,
            "new lua|the selected compactor returned without raising: replacement \
             compactors are deferred; compactors.fail is the only shipped policy",
        ),
    ];
    for (outcome, cancelled, expected) in cases {
        let mut machine = chatting(2, &list);
        assert_eq!(
            show(&machine.step(answered(overflow.clone()))),
            "compact compactor nil"
        );
        assert_eq!(show(&machine.step(called(outcome, cancelled))), expected);
    }
}

#[test]
fn an_input_its_phase_does_not_await_raises_internal() {
    let unexpected = "new internal|internal invariant violated: the models.loop step received \
                      an input its phase does not await";
    let (mut machine, _) = Machine::begin(2, MessageList::default(), "list", None, "compactor");
    assert_eq!(show(&machine.step(text("early"))), unexpected);
    assert_eq!(
        show(&machine.step(reply("late"))),
        unexpected,
        "a raise ends the call"
    );
}
