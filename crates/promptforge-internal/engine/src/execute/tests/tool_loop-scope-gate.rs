//! The loop's scope gate: a model call naming anything its round did not
//! advertise fails with `OutOfScopeToolCall`, which hints offered but
//! unscoped only when the name is the wire name of a tool the run offers.

use super::*;

/// The run offers `scoped` (id `tools/scoped`) and `global_tool` (id
/// `tools/global_tool`), and only `scoped` is in scope.
fn scoped_and_global_tools() -> FixtureTools {
    FixtureTools::new(
        vec![
            fixture_binding(
                "scoped",
                "A scoped tool.",
                Arc::new(ScopedFixtureTool::new("scoped", "A scoped tool.")),
            ),
            fixture_binding(
                "global_tool",
                "A global tool.",
                Arc::new(ScopedFixtureTool::new("global_tool", "A global tool.")),
            ),
        ],
        vec!["scoped".to_owned()],
    )
}

#[tokio::test(flavor = "current_thread")]
async fn model_calling_an_offered_but_unscoped_tool_is_a_hard_error() {
    // The loop's scope gate: a model call naming a tool the run offers but
    // the section never offered fails with OutOfScopeToolCall holding the
    // offered-but-unscoped hint.
    let gateway = ScriptedChat::new(vec![resp_tool_call(
        "call_1",
        "global_tool",
        "{\"value\":\"x\"}",
    )]);
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let (ctx, fixture) = loop_context(&prompt, scoped_and_global_tools());
    let error = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect_err("model calling a global-but-unscoped tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "global_tool");
            assert!(*global_exists, "the name is an offered tool's wire name");
            assert_eq!(in_scope, &["scoped".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    assert!(
        error
            .to_string()
            .ends_with("[\"scoped\"] (a catalog tool that was not offered in this section)"),
        "error message must hint offered-but-unscoped: {error}"
    );
    assert_eq!(gateway.call_count(), 1, "the rejected round is the last");
}

#[tokio::test(flavor = "current_thread")]
async fn model_calling_an_advertised_tool_by_canonical_id_gets_no_not_offered_suffix() {
    // A model reaches a tool only by the wire name its round advertised.
    // The canonical id of the advertised `scoped` is out of scope, but it
    // names no tool the section left unoffered, so the error drops the
    // offered-but-unscoped hint.
    let gateway = ScriptedChat::new(vec![resp_tool_call(
        "call_1",
        "tools/scoped",
        "{\"value\":\"x\"}",
    )]);
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let (ctx, fixture) = loop_context(&prompt, scoped_and_global_tools());
    let error = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect_err("model calling an advertised tool by its id must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "tools/scoped");
            assert!(!*global_exists, "an id is no offered tool's wire name");
            assert_eq!(in_scope, &["scoped".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    let message = error.to_string();
    assert!(
        message.ends_with("[\"scoped\"]"),
        "error message must end with the in-scope list: {message}"
    );
    assert!(
        !message.contains("not offered in this section"),
        "an advertised tool's id must not hint offered-but-unscoped: {message}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let gateway = ScriptedChat::new(vec![resp_tool_call(
        "call_1",
        "nonexistent",
        "{\"value\":\"x\"}",
    )]);
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let (ctx, fixture) = loop_context(&prompt, echo_tools());
    let error = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect_err("model calling a pure unknown tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "nonexistent");
            assert!(!*global_exists, "the name is no offered tool");
            assert_eq!(in_scope, &["echo".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    assert!(
        !error.to_string().contains("not offered in this section"),
        "pure unknown must not hint offered-but-unscoped: {error}"
    );
}
