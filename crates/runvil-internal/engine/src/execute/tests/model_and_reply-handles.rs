//! `models.get` and a handle's `infer`: a handle leaves the section
//! model alone, `models.infer` uses the section model and never binds
//! `reply`, `models.use` re-selection steers the next round, and a
//! section with no current model fails `models.infer` but still infers
//! through a handle, and a handle's `provider` reads the catalog's.

use super::*;
use crate::execute::tests::context::{bind_role, with_writer};
use crate::test_support::scenario::test_model;

/// The prompt declaring the `writer` and `analyst` roles and parking
/// `writer` as the prompt-wide default.
fn writer_and_analyst() -> PromptSource {
    PromptSource::new()
        .front("models: { writer: {}, analyst: {} }")
        .h1("models.default('writer')")
}

/// A run of `source` with the `writer` and `analyst` roles bound by hand to
/// `writer-model` and `analyst-model` and no prepare: the multi-model shape
/// the trivial fill cannot produce (it binds every role to the one current
/// model), exercising the runtime's label resolution directly.
#[track_caller]
fn bound_by_hand(source: &PromptSource) -> Scenario {
    Scenario::new(source.build())
        .unprepared()
        .context(bind_role("writer", test_model("writer-model")))
        .context(bind_role("analyst", test_model("analyst-model")))
}

#[test]
fn models_get_returns_a_handle_without_changing_the_section_model() {
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let source = writer_and_analyst().prose(
        "Only",
        "
            ```lua
            store.write('handle.txt', models.get('analyst').name)
            ```

            Ask the model.

            ```lua
            return models.infer(prose)
            ```",
    );
    let trace = bound_by_hand(&source).chat(&gateway).run();

    assert_eq!(trace.text(), "hello from the mock");
    assert_eq!(
        trace.read("handle.txt").unwrap(),
        "analyst",
        "models.get must return the analyst handle"
    );
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(
        body.options.model(),
        "writer-model",
        "models.get must not change the section's model"
    );
}

#[test]
fn models_infer_uses_the_section_model_without_touching_reply() {
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let source = with_writer(PromptSource::new()).prose(
        "Only",
        "
        ```lua
        var.r = models.infer('ping')
        ```

        ```lua
        return var.r .. ':' .. tostring(reply)
        ```",
    );
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "pong:nil",
        "models.infer must not bind the section's reply"
    );

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body.options.model(), "claude-sonnet-4-6");
    assert!(
        body.tools.is_empty(),
        "models.infer advertises no tools: {body:?}"
    );
    assert_eq!(
        body.messages.len(),
        1,
        "models.infer runs on a fresh context: {body:?}"
    );
}

#[test]
fn handle_infer_uses_that_model_regardless_of_the_section_model() {
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let source = writer_and_analyst().section("Only", "return models.get('analyst'):infer('ping')");
    let trace = bound_by_hand(&source).chat(&gateway).run();
    assert_eq!(trace.text(), "pong");
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(
        body.options.model(),
        "analyst-model",
        "a handle's infer must use the handle's model, not the section default"
    );
}

#[test]
fn models_use_reselection_steers_the_next_round() {
    let gateway = ScriptedChat::new(vec![text("first"), text("second")]);
    let source = writer_and_analyst().section(
        "Only",
        "
        models.use('writer')
        models.infer('ping')
        models.use('analyst')
        return models.infer('ping')",
    );
    let trace = bound_by_hand(&source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "second",
        "re-selection within a section must succeed"
    );
    let requests = gateway.requests();
    assert_eq!(
        requests.len(),
        2,
        "both infer rounds must reach the gateway"
    );
    assert_eq!(
        requests[0].options.model(),
        "writer-model",
        "the first round uses the initial selection"
    );
    assert_eq!(
        requests[1].options.model(),
        "analyst-model",
        "the second round uses the re-selected model"
    );
}

/// The prompt declaring only the `analyst` role, with no prompt-wide
/// default, whose `Only` section runs `lua`.
fn analyst_only(lua: &str) -> String {
    PromptSource::new()
        .front("models: { analyst: {} }")
        .section("Only", lua)
        .build()
}

#[test]
fn models_infer_without_use_or_default_errors() {
    let trace = Scenario::new(analyst_only("return models.infer('ping')"))
        .unprepared()
        .context(bind_role("analyst", test_model("analyst-model")))
        .run();
    let error = trace.error();
    assert!(
        error
            .to_string()
            .contains("model binding required for section Only"),
        "the error must name the section: {error}"
    );
}

#[test]
fn a_handles_provider_is_the_catalog_provider_or_nil() {
    let source = PromptSource::new()
        .front("models: { writer: {}, analyst: {} }")
        .section(
            "Only",
            "return tostring(models.get('writer').provider) .. ':' .. tostring(models.get('analyst').provider)",
        );
    let writer = test_model("writer-model").with_provider("xai");
    let trace = Scenario::new(source.build())
        .unprepared()
        .context(bind_role("writer", writer))
        .context(bind_role("analyst", test_model("analyst-model")))
        .run();
    assert_eq!(trace.text(), "xai:nil");
}

#[test]
fn models_get_infer_works_without_any_section_model() {
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let trace = Scenario::new(analyst_only("return models.get('analyst'):infer('ping')"))
        .unprepared()
        .context(bind_role("analyst", test_model("analyst-model")))
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "pong");
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body.options.model(), "analyst-model");
}
