//! The Lua loop's instruction cost. `models.loop` now runs on the author's
//! Lua instruction budget: every instruction the shim spends per round is
//! one the every-Nth-instruction hook counts, so a round must cost a few
//! hundred instructions of shim bookkeeping, never thousands. That keeps
//! the cancel poll's cadence in rounds where the Rust loop left it and
//! stops the loop from taxing an author's block for work the host used to
//! do for free.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{HookTriggers, MultiValue, Thread, Value, VmState};
use serde_json::json;

use promptforge_api_types::events::ToolCallEvent;
use promptforge_lua::Error;

use crate::execute::protocol::{Answer, ChatResult, Request, ToolCallOutcome, YieldParse};
use crate::lua::SectionVm;

use super::{scheduler_vm_with_tools, test_models, test_tools};

/// The most Lua instructions one model-tool round may spend inside the
/// loop shim: from one `chat` yield to the next, through the tool-call
/// answer, the `tool_call` yield, the result, and both record appends.
/// "A few hundred" is the budget; the shim measured 86 per round when the
/// ceiling was set, so a change that triples the round overhead trips
/// this while ordinary edits do not.
const ROUND_INSTRUCTION_CEILING: u64 = 300;

/// The fewest Lua instructions a round can honestly spend in the shim:
/// draining notices, reading the answer, yielding the tool call, and
/// appending the assistant and tool records is a few dozen instructions
/// at the least. A round under this floor means the counted span is not
/// the loop's work at all (the loop moved back into Rust, or the hook is
/// not firing on the thread), and the test would otherwise pass while
/// showing nothing about the quota.
const ROUND_INSTRUCTION_FLOOR: u64 = 20;

/// Rounds measured after the first, so the assertion reads steady-state
/// cost rather than the one-time argument decode.
const MEASURED_ROUNDS: i64 = 3;

/// Parses one yielded request table, failing on anything malformed.
fn parse_request(vm: &SectionVm, yielded: MultiValue) -> Request {
    let value = yielded.into_iter().next().expect("one yielded value");
    match Request::from_yield(vm.lua(), &value) {
        YieldParse::Request(request) => request,
        other => panic!("the shim yield is a well-formed request, got {other:?}"),
    }
}

/// Renders `answer` as the shim's envelope and resumes the loop with it.
fn resume_with(vm: &SectionVm, thread: &Thread, answer: Answer<Error>) -> MultiValue {
    let (envelope, _retained) = answer
        .into_envelope(vm.lua())
        .expect("the envelope renders");
    thread
        .resume::<MultiValue>(envelope)
        .expect("the loop accepts the answer")
}

/// A completed round that requested one `echo` call.
fn tool_call_round() -> Answer<Error> {
    Answer::Chat(Ok(Box::new(ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: Some(vec![ToolCallEvent {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "value": "hi" }),
        }]),
        finish_reason: Some("tool_calls".to_owned()),
        model: "test-model".to_owned(),
        metrics: None,
    })))
}

/// A completed round that produced the terminal reply.
fn reply_round(text: &str) -> Answer<Error> {
    Answer::Chat(Ok(Box::new(ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: Some(text.to_owned()),
        empty_detail: None,
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: "test-model".to_owned(),
        metrics: None,
    })))
}

#[test]
fn a_models_loop_round_costs_a_few_hundred_lua_instructions() {
    // The block is the bench prompt's shape: one user message, one loop
    // call. The coroutine carries a per-instruction counting hook in
    // place of the VM's cancel hook, so the counter reads exactly the Lua
    // instructions the shim executes between two yields. Every round is
    // answered with one tool call so the measured span is the full
    // model-tool round, not the terminal exit.
    let vm = scheduler_vm_with_tools(&test_models(), &test_tools(), None);
    let function = vm
        .lua()
        .load("local msgs = messages.new()\nmsgs:user('hi')\nmodels.loop(msgs)\nreturn #msgs")
        .into_function()
        .expect("the loop block compiles");
    let thread = vm
        .lua()
        .create_thread(function)
        .expect("the loop thread creates");
    let executed = Arc::new(AtomicU64::new(0));
    thread
        .set_hook(HookTriggers::new().every_nth_instruction(1), {
            let executed = Arc::clone(&executed);
            move |_lua, _debug| {
                executed.fetch_add(1, Ordering::Relaxed);
                Ok(VmState::Continue)
            }
        })
        .expect("the counting hook installs on the loop thread");

    let yielded = thread
        .resume::<MultiValue>(())
        .expect("the block yields its first chat");
    assert!(
        matches!(parse_request(&vm, yielded), Request::Chat { .. }),
        "the loop's first yield is a chat request"
    );

    // One mark per chat yield: the difference between consecutive marks
    // is one round's shim cost.
    let mut marks = vec![executed.load(Ordering::Relaxed)];
    for _ in 0..=MEASURED_ROUNDS {
        let yielded = resume_with(&vm, &thread, tool_call_round());
        match parse_request(&vm, yielded) {
            Request::ToolCall { alias, call_id, .. } => {
                assert_eq!(alias, "echo");
                assert_eq!(call_id.as_deref(), Some("call_1"));
            }
            other => panic!("the loop yields the model's tool call, got {other:?}"),
        }
        let yielded = resume_with(
            &vm,
            &thread,
            Answer::ToolCallResult(Ok(ToolCallOutcome::Plain("echoed".to_owned()))),
        );
        assert!(
            matches!(parse_request(&vm, yielded), Request::Chat { .. }),
            "after the tool result the loop yields the next chat"
        );
        marks.push(executed.load(Ordering::Relaxed));
    }

    let returned = resume_with(&vm, &thread, reply_round("done"));
    // user + (assistant tool-call record + tool record) per round + terminal.
    let expected_len = 1 + 2 * (MEASURED_ROUNDS + 1) + 1;
    assert_eq!(
        returned.into_iter().next(),
        Some(Value::Integer(expected_len)),
        "the loop appended every round's records and the terminal reply"
    );

    let costs: Vec<u64> = marks.windows(2).map(|pair| pair[1] - pair[0]).collect();
    for (round, cost) in costs.iter().enumerate().skip(1) {
        assert!(
            *cost >= ROUND_INSTRUCTION_FLOOR,
            "round {round} spent {cost} Lua instructions in the loop shim, under the \
             {ROUND_INSTRUCTION_FLOOR} floor: the loop's work is not being counted; \
             per-round costs: {costs:?}"
        );
        assert!(
            *cost <= ROUND_INSTRUCTION_CEILING,
            "round {round} spent {cost} Lua instructions in the loop shim, over the \
             {ROUND_INSTRUCTION_CEILING} ceiling; per-round costs: {costs:?}"
        );
    }
}
