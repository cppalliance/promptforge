use mlua::Lua;
use promptforge_model_client::client::Message;
use serde_json::{Value, json};

use super::{
    Compactor, OverflowReason, install_compactors, invoke_selected, is_context_overflow, precheck,
};
use crate::Error;

fn lua_with_compactors() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    install_compactors(&lua, &globals).expect("compactors install cannot fail on a fresh VM");
    lua
}

/// Extracts the typed crate error a `compactors.fail` raise carried across
/// the Lua boundary (LUA-012: the typed error, never its flattened text).
/// mlua wraps a callback's error in `CallbackError` for the traceback; the
/// original external error rides as its cause.
fn raised_crate_error_ref(error: &mlua::Error) -> &Error {
    let cause = match error {
        mlua::Error::CallbackError { cause, .. } => cause.as_ref(),
        other => other,
    };
    match cause {
        mlua::Error::ExternalError(cause) => cause
            .downcast_ref::<Error>()
            .expect("the raise must carry the typed crate error, not text"),
        other => panic!("expected an ExternalError carrying the typed error, got {other:?}"),
    }
}

/// The overflow reason a raise carried, when it was a context exhaustion.
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
fn fail_invocation_raises_typed_exhaustion_carrying_the_reason() {
    for reason in [OverflowReason::Precheck, OverflowReason::Provider] {
        let error = Compactor::Fail.invoke(reason);
        match error {
            Error::ContextExhausted { reason: carried } => {
                assert_eq!(
                    carried, reason,
                    "the exhaustion must carry the invoking reason"
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
            "the Lua raise must carry the typed exhaustion with the invocation reason"
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
fn invoke_selected_defaults_to_fail_without_a_callback() {
    let lua = lua_with_compactors();
    for reason in [OverflowReason::Precheck, OverflowReason::Provider] {
        match invoke_selected(&lua, None, reason) {
            Error::ContextExhausted { reason: carried } => {
                assert_eq!(carried, reason, "the default carries the invoking reason");
            }
            other => panic!("expected ContextExhausted, got {other:?}"),
        }
    }
}

#[test]
fn invoke_selected_invokes_the_callback_with_the_reason_tag() {
    let lua = lua_with_compactors();
    let fail: mlua::Function = lua
        .load("compactors.fail")
        .eval()
        .expect("compactors.fail evaluates");
    let key = lua
        .create_registry_value(fail)
        .expect("the stash cannot fail");
    for reason in [OverflowReason::Precheck, OverflowReason::Provider] {
        match invoke_selected(&lua, Some(&key), reason) {
            Error::ContextExhausted { reason: carried } => {
                assert_eq!(
                    carried, reason,
                    "the callback's typed raise crosses back with the invoking reason"
                );
            }
            other => panic!("expected ContextExhausted, got {other:?}"),
        }
    }
}

#[test]
fn invoke_selected_rejects_a_compactor_that_returns() {
    let lua = lua_with_compactors();
    let returns: mlua::Function = lua
        .load("function(reason) return { role = 'user', content = 'summary' } end")
        .eval()
        .expect("the returning compactor evaluates");
    let key = lua
        .create_registry_value(returns)
        .expect("the stash cannot fail");
    match invoke_selected(&lua, Some(&key), OverflowReason::Precheck) {
        Error::Lua(message) => assert!(
            message.contains("deferred") && message.contains("compactors.fail"),
            "a returned replacement names the deferred framework, got: {message}"
        ),
        other => panic!("expected the deferred-replacement Lua error, got {other:?}"),
    }
}

#[test]
fn invoke_selected_flattens_a_compactors_own_untyped_raise() {
    let lua = lua_with_compactors();
    let raises: mlua::Function = lua
        .load("function(reason) error('custom failure: ' .. reason, 0) end")
        .eval()
        .expect("the raising compactor evaluates");
    let key = lua
        .create_registry_value(raises)
        .expect("the stash cannot fail");
    match invoke_selected(&lua, Some(&key), OverflowReason::Provider) {
        Error::LuaRuntime { message, .. } => assert!(
            message.contains("custom failure: provider"),
            "the compactor's own error survives with the reason tag, got: {message}"
        ),
        other => panic!("expected the compactor's own runtime error, got {other:?}"),
    }
}

#[test]
fn precheck_passes_a_request_within_the_window() {
    let context = std::num::NonZeroU32::new(4096).expect("non-zero");
    let messages = vec![Message::user("a short prompt")];
    precheck(&messages, context).expect("a small request fits the window");
}

#[test]
fn precheck_overflows_a_request_past_the_window() {
    let context = std::num::NonZeroU32::new(16).expect("non-zero");
    let messages = vec![Message::user("x".repeat(4096))];
    let reason = precheck(&messages, context).expect_err("a huge request cannot fit a tiny window");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn precheck_boundary_admits_an_exact_fit() {
    // One message of 396 characters estimates to 396/4 + 4 overhead = 103
    // tokens: a 103-token window admits it exactly, 102 refuses it.
    let messages = vec![Message::user("x".repeat(396))];
    let exact = std::num::NonZeroU32::new(103).expect("non-zero");
    precheck(&messages, exact).expect("an exact-fit estimate is admitted");
    let under = std::num::NonZeroU32::new(102).expect("non-zero");
    let reason = precheck(&messages, under).expect_err("one token under the estimate refuses");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn precheck_counts_tool_call_arguments_and_part_text() {
    let context = std::num::NonZeroU32::new(16).expect("non-zero");
    // An assistant tool-call turn whose visible text is empty still carries
    // its arguments onto the wire; the estimate must count them.
    let calls = vec![Message::from_validated_parts(
        "assistant",
        Value::String(String::new()),
        None,
        Some(vec![json!({
            "id": "call_1",
            "name": "echo",
            "arguments": "x".repeat(4096),
        })]),
    )];
    let reason =
        precheck(&calls, context).expect_err("tool-call arguments count toward the estimate");
    assert_eq!(reason, OverflowReason::Precheck);
    // A multimodal parts array contributes its text parts.
    let parts = vec![Message::from_validated_parts(
        "user",
        json!([{ "type": "text", "text": "x".repeat(4096) }]),
        None,
        None,
    )];
    let reason = precheck(&parts, context).expect_err("part text counts toward the estimate");
    assert_eq!(reason, OverflowReason::Precheck);
}

#[test]
fn provider_overflow_detection_matches_known_signatures() {
    let cases: &[(u16, &str, bool)] = &[
        (
            400,
            "This model's maximum context length is 4096 tokens.",
            true,
        ),
        (400, "context_length_exceeded", true),
        (400, "the request exceeds the available context size", true),
        (413, "prompt is too long", true),
        (400, "CONTEXT WINDOW exceeded", true),
        (400, "too many tokens in prompt", true),
        // A server fault never classifies, even with overflow wording.
        (500, "maximum context length is 4096 tokens", false),
        // A client rejection without overflow wording stays a plain error.
        (400, "invalid request: unknown field `stream`", false),
        (401, "context length", false),
    ];
    for (status, body, expected) in cases {
        assert_eq!(
            is_context_overflow(*status, body),
            *expected,
            "status {status} with body {body:?}"
        );
    }
}
