//! The Lua loop's instruction cost. `models.loop` now runs on the author's
//! Lua instruction budget: every instruction the shim spends per round is
//! one the every-Nth-instruction hook counts, so a round must cost a few
//! hundred instructions of shim bookkeeping, never thousands. That keeps
//! the cancel poll's cadence in rounds where the Rust loop left it and
//! stops the loop from taxing an author's block for work the host used to
//! do for free.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{HookTriggers, MultiValue, Value, VmState};
use serde_json::json;

use promptforge_lua::Error;
use promptforge_types::metrics::ToolCallEvent;

use crate::execute::protocol::{Answer, ChatResult, Request, ToolCallOutcome};

use super::{parse_request, resume_with, scheduler_vm_with_tools, test_models, test_tools};

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
        turn: 1,
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
        turn: 2,
    })))
}

#[test]
fn a_models_loop_round_costs_a_few_hundred_lua_instructions() {
    // The block is the bench prompt's shape: one user message, one loop
    // call. The coroutine runs a per-instruction counting hook in
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

    // Every round opens with the notice drain, answered empty here, then
    // the chat.
    let yielded = thread
        .resume::<MultiValue>(())
        .expect("the block yields its first drain");
    assert!(
        matches!(parse_request(&vm, yielded), Request::DrainTaskNotices),
        "the loop's first yield drains the task notices"
    );
    let yielded = resume_with(&vm, &thread, Answer::DrainTaskNotices(Ok(Vec::new())));
    assert!(
        matches!(parse_request(&vm, yielded), Request::Chat { .. }),
        "after the drain the loop yields its first chat"
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
            matches!(parse_request(&vm, yielded), Request::DrainTaskNotices),
            "after the tool result the loop drains notices ahead of the next chat"
        );
        let yielded = resume_with(&vm, &thread, Answer::DrainTaskNotices(Ok(Vec::new())));
        assert!(
            matches!(parse_request(&vm, yielded), Request::Chat { .. }),
            "after the drain the loop yields the next chat"
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
