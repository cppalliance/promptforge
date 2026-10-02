//! Tests for the `compactors` namespace and the context-window precheck.

use std::num::NonZeroU32;

use mlua::Lua;
use promptforge_model_client::client::Message;
use promptforge_model_client::detail::message_from_validated_parts;
use promptforge_types::metrics::Usage;
use serde_json::{Value, json};

use super::{Compactor, OverflowReason, UsageAnchor, install_compactors, output_reserve, precheck};
use crate::Error;

fn lua_with_compactors() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    install_compactors(&lua, &globals).expect("compactors install cannot fail on a fresh VM");
    lua
}

/// Extracts the typed crate error a `compactors.fail` raise brought across
/// the Lua boundary (the typed error, never its flattened text).
/// mlua wraps a callback's error in `CallbackError` for the traceback; the
/// original external error is kept as its cause.
fn raised_crate_error_ref(error: &mlua::Error) -> &Error {
    let cause = match error {
        mlua::Error::CallbackError { cause, .. } => cause.as_ref(),
        other => other,
    };
    match cause {
        mlua::Error::ExternalError(cause) => cause
            .downcast_ref::<Error>()
            .expect("the raise must hold the typed crate error, not text"),
        other => panic!("expected an ExternalError holding the typed error, got {other:?}"),
    }
}

/// The overflow reason a raise held, when it was a context exhaustion.
fn raised_reason(error: &mlua::Error) -> OverflowReason {
    match raised_crate_error_ref(error) {
        Error::ContextExhausted { reason } => *reason,
        other => panic!("expected ContextExhausted, got {other:?}"),
    }
}

#[test]
fn the_omitted_compactor_defaults_to_fail() {
    // The optional compactor parameter's default: omitting it selects
    // `compactors.fail`, the only shipped policy.
    // Resolve through a call boundary, exactly as the loop's optional
    // compactor parameter resolves.
    fn resolve(selected: Option<Compactor>) -> Compactor {
        selected.unwrap_or_default()
    }
    assert_eq!(Compactor::default(), Compactor::Fail);
    assert_eq!(resolve(None), Compactor::Fail);
    let error = Compactor::Fail.invoke(OverflowReason::Provider);
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: OverflowReason::Provider
            }
        ),
        "the default policy must raise typed context exhaustion, got {error:?}"
    );
}

#[test]
fn fail_invocation_raises_typed_exhaustion_holding_the_reason() {
    for reason in [OverflowReason::Precheck, OverflowReason::Provider] {
        let error = Compactor::Fail.invoke(reason);
        match error {
            Error::ContextExhausted { reason: raised } => {
                assert_eq!(
                    raised, reason,
                    "the exhaustion must hold the invoking reason"
                );
            }
            other => panic!("expected ContextExhausted, got {other:?}"),
        }
    }
    let precheck = Compactor::Fail.invoke(OverflowReason::Precheck).to_string();
    let provider = Compactor::Fail.invoke(OverflowReason::Provider).to_string();
    assert_ne!(
        precheck, provider,
        "the two reasons must render distinct diagnostics"
    );
    assert!(
        precheck.starts_with("context exhausted: "),
        "the exhaustion names itself first: {precheck}"
    );
}

#[test]
fn compactors_fail_raises_typed_exhaustion_from_lua() {
    let lua = lua_with_compactors();
    for (tag, expected) in [
        ("precheck", OverflowReason::Precheck),
        ("provider", OverflowReason::Provider),
    ] {
        let error = lua
            .load(format!("compactors.fail('{tag}')"))
            .exec()
            .expect_err("compactors.fail always raises");
        assert_eq!(
            raised_reason(&error),
            expected,
            "the Lua raise must hold the typed exhaustion with the invocation reason"
        );
    }
}

#[test]
fn compactors_fail_rejects_an_unknown_reason() {
    let lua = lua_with_compactors();
    let error = lua
        .load("compactors.fail('bogus')")
        .exec()
        .expect_err("an unknown reason tag is an argument error");
    let message = error.to_string();
    assert!(
        message.contains("bogus"),
        "the argument error must name the rejected tag: {message}"
    );
    assert!(
        !matches!(
            raised_crate_error_ref(&error),
            Error::ContextExhausted { .. }
        ),
        "a bad tag is an authoring error, never context exhaustion"
    );
}

#[test]
fn the_fail_policy_invokes_as_typed_exhaustion_for_either_reason() {
    for reason in [OverflowReason::Precheck, OverflowReason::Provider] {
        match Compactor::Fail.invoke(reason) {
            Error::ContextExhausted { reason: raised } => {
                assert_eq!(raised, reason, "the policy holds the invoking reason");
            }
            other => panic!("expected ContextExhausted, got {other:?}"),
        }
    }
}

fn window(tokens: u32) -> NonZeroU32 {
    NonZeroU32::new(tokens).expect("a test window is non-zero")
}

/// The reserve for a `context`-token window whose binding sets `max_tokens`.
fn reserve(context: u32, max_tokens: Option<u32>) -> u32 {
    output_reserve(window(context), max_tokens.map(window))
}

#[test]
fn precheck_passes_a_request_within_the_window() {
    let messages = vec![Message::user("a short prompt")];
    precheck(&messages, window(4096), reserve(4096, None), None)
        .expect("a small request fits the window");
}

#[test]
fn precheck_overflows_a_request_past_the_window() {
    let messages = vec![Message::user("x".repeat(4096))];
    let reason = precheck(&messages, window(16), 0, None)
        .expect_err("a huge request cannot fit a tiny window");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn precheck_boundary_admits_an_exact_fit() {
    // One message of 396 characters estimates to 396/4 + 4 overhead = 103
    // tokens. A `max_tokens` of 20 reserves 20, so a 123-token window
    // admits it exactly and 122 refuses it.
    let messages = vec![Message::user("x".repeat(396))];
    precheck(&messages, window(123), reserve(123, Some(20)), None)
        .expect("a count plus reserve equal to the window is admitted");
    let reason = precheck(&messages, window(122), reserve(122, Some(20)), None)
        .expect_err("one token under the count plus reserve refuses");
    assert_eq!(reason, OverflowReason::Precheck);
    // With no reserve the old boundary stands: 103 admits, 102 refuses.
    precheck(&messages, window(103), 0, None).expect("an exact-fit estimate is admitted");
    precheck(&messages, window(102), 0, None).expect_err("one token under the estimate refuses");
}

#[test]
fn the_default_reserve_moves_the_boundary_by_an_eighth_of_the_window() {
    // A 117-token window reserves 117 / 8 = 14 tokens, so the 103-token
    // estimate plus the reserve fits it exactly; 116 reserves the same 14
    // and falls one short.
    let messages = vec![Message::user("x".repeat(396))];
    precheck(&messages, window(117), reserve(117, None), None)
        .expect("count plus the default reserve equal to the window is admitted");
    let reason = precheck(&messages, window(116), reserve(116, None), None)
        .expect_err("one token under the count plus the default reserve refuses");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn the_reserve_is_the_bindings_max_tokens_when_it_is_set() {
    assert_eq!(reserve(4096, Some(300)), 300);
    assert_eq!(reserve(4096, Some(2048)), 2048);
    assert_eq!(reserve(131_072, Some(20_000)), 20_000);
}

#[test]
fn the_default_reserve_is_an_eighth_of_the_window_capped_at_8192() {
    assert_eq!(reserve(4096, None), 512);
    assert_eq!(reserve(8192, None), 1024);
    assert_eq!(reserve(65_535, None), 8191);
    assert_eq!(reserve(65_536, None), 8192);
    assert_eq!(reserve(131_072, None), 8192);
    assert_eq!(reserve(1_000_000, None), 8192);
    assert_eq!(reserve(7, None), 0);
}

#[test]
fn the_reserve_never_exceeds_half_the_window() {
    assert_eq!(reserve(100, Some(50)), 50);
    assert_eq!(reserve(100, Some(51)), 50);
    assert_eq!(reserve(100, Some(100)), 50);
    assert_eq!(reserve(100, Some(1_000_000)), 50);
    assert_eq!(reserve(101, Some(101)), 50);
    assert_eq!(reserve(1, Some(5)), 0);
}

#[test]
fn a_max_tokens_at_or_above_the_window_still_admits_an_empty_request() {
    for max_tokens in [100, 101, 5000] {
        precheck(&[], window(100), reserve(100, Some(max_tokens)), None)
            .expect("the half-window cap leaves an empty request room");
    }
    let small = vec![Message::user("hi")];
    precheck(&small, window(100), reserve(100, Some(100)), None)
        .expect("a small request still fits beside the cap");
}

/// The request a served round carried: one 396-character user message,
/// estimated at 103 tokens.
fn sent_request() -> Vec<Message> {
    vec![Message::user("x".repeat(396))]
}

/// The next round's request: the sent message, the reply the model gave
/// (`reply_chars` characters), and a new user message of `tail_chars`.
fn next_request(reply_chars: usize, tail_chars: usize) -> Vec<Message> {
    vec![
        Message::user("x".repeat(396)),
        Message::assistant("y".repeat(reply_chars)),
        Message::user("z".repeat(tail_chars)),
    ]
}

fn usage(prompt: u32, completion: u32, reasoning: Option<u32>) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        cached_tokens: None,
        reasoning_tokens: reasoning,
    }
}

/// Whether `messages` fit `context` with no reserve beside `anchor`.
fn admits(messages: &[Message], context: u32, anchor: Option<&UsageAnchor>) -> bool {
    precheck(messages, window(context), 0, anchor).is_ok()
}

#[test]
fn the_anchor_counts_the_reported_total_and_not_the_reply_twice() {
    // The provider counted 1000 prompt and 50 completion tokens, so the
    // anchor is 1050 and already covers the 4000-character reply. The
    // 40-character tail adds 10 + 4, so the count is 1064; counting the
    // reply again would add 1004 and refuse the 1064-token window.
    let anchor = UsageAnchor::new(sent_request(), &usage(1000, 50, None));
    assert_eq!(anchor.tokens(), 1050);
    let next = next_request(4000, 40);
    assert!(admits(&next, 1064, Some(&anchor)));
    assert!(!admits(&next, 1063, Some(&anchor)));
    assert!(
        !admits(&next, 1064, None),
        "the full estimate of the same request is 1121 tokens"
    );
    assert!(admits(&next, 1121, None));
}

#[test]
fn the_anchor_leaves_the_reserve_on_top_of_its_count() {
    let anchor = UsageAnchor::new(sent_request(), &usage(1000, 50, None));
    let next = next_request(4000, 40);
    precheck(&next, window(1164), 100, Some(&anchor))
        .expect("an anchored count plus the reserve equal to the window is admitted");
    let reason = precheck(&next, window(1163), 100, Some(&anchor))
        .expect_err("one token under the anchored count plus the reserve refuses");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn reported_reasoning_tokens_come_off_the_anchor() {
    // The history never resends reasoning, so 30 of the 50 completion
    // tokens do not count: 1000 + 50 - 30 = 1020, and the count is 1034.
    let anchor = UsageAnchor::new(sent_request(), &usage(1000, 50, Some(30)));
    assert_eq!(anchor.tokens(), 1020);
    let next = next_request(4000, 40);
    assert!(admits(&next, 1034, Some(&anchor)));
    assert!(!admits(&next, 1033, Some(&anchor)));
    let more_than_all = UsageAnchor::new(sent_request(), &usage(5, 5, Some(100)));
    assert_eq!(
        more_than_all.tokens(),
        0,
        "reasoning above the total saturates at zero"
    );
}

#[test]
fn an_anchor_fits_a_request_the_full_estimate_would_refuse() {
    // A provider that counts far fewer tokens than chars/4 lets a long
    // conversation through: the tiny anchor admits where the estimate
    // of 1121 tokens refuses a 200-token window.
    let anchor = UsageAnchor::new(sent_request(), &usage(10, 5, None));
    let next = next_request(4000, 40);
    assert!(admits(&next, 200, Some(&anchor)));
    assert!(!admits(&next, 200, None));
}

#[test]
fn a_different_prefix_falls_back_to_the_full_estimate() {
    let anchor = UsageAnchor::new(sent_request(), &usage(10, 5, None));
    let matching = next_request(4000, 40);
    assert!(admits(&matching, 200, Some(&anchor)), "the control anchors");

    // A rewritten first message.
    let mut rewritten = next_request(4000, 40);
    rewritten[0] = Message::user("w".repeat(396));
    assert!(!admits(&rewritten, 200, Some(&anchor)));
    // The same text under another role.
    let mut other_role = next_request(4000, 40);
    other_role[0] = Message::assistant("x".repeat(396));
    assert!(!admits(&other_role, 200, Some(&anchor)));
    // A compacted history: a summary replaced the sent message, and the
    // reply still sits at index `n`.
    let mut compacted = next_request(4000, 40);
    compacted[0] = Message::user("summary");
    assert!(!admits(&compacted, 200, Some(&anchor)));
    // A merged history: the projection folded the reply into the sent
    // message, so the first message is no longer the one that was sent.
    let merged = vec![
        Message::user(format!("{}{}", "x".repeat(396), "y".repeat(4000))),
        Message::user("z".repeat(40)),
        Message::assistant("a"),
    ];
    assert!(!admits(&merged, 200, Some(&anchor)));
}

#[test]
fn a_request_with_no_reply_after_the_prefix_falls_back_to_the_full_estimate() {
    let anchor = UsageAnchor::new(sent_request(), &usage(10, 5, None));
    // Exactly the sent request again: nothing follows the prefix, so
    // message `n` does not exist. Its estimate of 103 tokens is refused by
    // a 100-token window, while the anchor alone (15) would pass.
    assert!(!admits(&sent_request(), 100, Some(&anchor)));
    // Fewer messages than were sent: the anchor covers three messages,
    // the request holds one.
    let longer = UsageAnchor::new(next_request(4000, 40), &usage(10, 5, None));
    assert!(!admits(&sent_request(), 100, Some(&longer)));
}

#[test]
fn a_message_after_the_prefix_that_is_not_the_assistants_reply_falls_back() {
    let anchor = UsageAnchor::new(sent_request(), &usage(10, 5, None));
    let user_after = vec![
        Message::user("x".repeat(396)),
        Message::user("y".repeat(4000)),
        Message::user("z".repeat(40)),
    ];
    assert!(!admits(&user_after, 200, Some(&anchor)));
    let tool_after = vec![
        Message::user("x".repeat(396)),
        Message::tool("call_1", "y".repeat(4000)),
    ];
    assert!(!admits(&tool_after, 200, Some(&anchor)));
}

#[test]
fn precheck_counts_tool_call_arguments_and_part_text() {
    let context = window(16);
    // An assistant tool-call turn whose visible text is empty still sends
    // its arguments onto the wire; the estimate must count them.
    let calls = vec![message_from_validated_parts(
        "assistant",
        Value::String(String::new()),
        None,
        Some(vec![json!({
            "id": "call_1",
            "name": "echo",
            "arguments": "x".repeat(4096),
        })]),
    )];
    let reason = precheck(&calls, context, 0, None)
        .expect_err("tool-call arguments count toward the estimate");
    assert_eq!(reason, OverflowReason::Precheck);
    // A multimodal parts array contributes its text parts.
    let parts = vec![message_from_validated_parts(
        "user",
        json!([{ "type": "text", "text": "x".repeat(4096) }]),
        None,
        None,
    )];
    let reason =
        precheck(&parts, context, 0, None).expect_err("part text counts toward the estimate");
    assert_eq!(reason, OverflowReason::Precheck);
}
