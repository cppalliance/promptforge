//! The loop's scope gate: a model call naming anything its round did not
//! advertise fails with `OutOfScopeToolCall`, which hints offered but
//! unscoped only when the name is the wire name of a tool the run offers.

use super::*;

/// A run of the loop section that offers `tools/scoped` (wire name
/// `scoped`) and leaves `tools/global_tool` (wire name `global_tool`)
/// unoffered, though the run offers both.
fn scoped_loop() -> Scenario {
    let source = one_section(&format!("tools.offer('tools/scoped')\n{LOOP_TO_TEXT}"));
    loop_scenario(source)
        .offer("scoped", scoped_tool("scoped", "A scoped tool.").0)
        .offer(
            "global_tool",
            scoped_tool("global_tool", "A global tool.").0,
        )
}

#[test]
fn model_calling_an_offered_but_unscoped_tool_is_a_hard_error() {
    // The loop's scope gate: a model call naming a tool the run offers but
    // the section never offered fails with OutOfScopeToolCall holding the
    // offered-but-unscoped hint.
    let gateway = ScriptedChat::new(vec![calls(&[(
        "call_1",
        "global_tool",
        "{\"value\":\"x\"}",
    )])]);
    let trace = scoped_loop().chat(&gateway).run();
    let error = trace.error().inner();
    match error {
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

#[test]
fn model_calling_an_advertised_tool_by_canonical_id_gets_no_not_offered_suffix() {
    // A model reaches a tool only by the wire name its round advertised.
    // The canonical id of the advertised `scoped` is out of scope, but it
    // names no tool the section left unoffered, so the error drops the
    // offered-but-unscoped hint.
    let gateway = ScriptedChat::new(vec![calls(&[(
        "call_1",
        "tools/scoped",
        "{\"value\":\"x\"}",
    )])]);
    let trace = scoped_loop().chat(&gateway).run();
    let error = trace.error().inner();
    match error {
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

#[test]
fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let gateway = ScriptedChat::new(vec![calls(&[(
        "call_1",
        "nonexistent",
        "{\"value\":\"x\"}",
    )])]);
    let source = one_section(&format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    let error = trace.error().inner();
    match error {
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
