//! Tests for section model selection and the prologue and epilog phases.
//! The `sys.model`, `item`, and `reply` reads sit in `globals`, and the
//! `models.get` and handle `infer` cases in `handles`.

use promptforge_parser::test_support::{PromptSource, parse_ok};

use crate::test_support::scenario::{Trace, test_model};

use super::context::{bind_role, with_writer};
use super::*;

/// The run's observations as section and detail pairs, from its start on:
/// the parse events before it are left out.
fn run_events(trace: &Trace) -> Vec<(String, String)> {
    trace
        .records()
        .into_iter()
        .skip_while(|record| record.detail != detail::RUN_STARTED.to_string())
        .map(|record| (record.section, record.detail))
        .collect()
}

#[test]
fn models_use_forwards_binding_completion_options_to_the_gateway() {
    // models.use -> completion_options -> GatewayChat::complete must set
    // the binding's model, the hard-keyword thinking switch, and the
    // section's `models.use` sampling options on the chat body. Roles
    // declare no sampling fields, so a section on the prompt-wide default
    // sends none.
    let gateway = ScriptedChat::new(vec![text("hello from the mock"), text("hello again")]);
    let source = PromptSource::new()
        .front("models: { analyst: { keywords: [no-thinking] } }")
        .h1("models.default('analyst')")
        .prose(
            "Only",
            "
            ```lua
            models.use('analyst', { temperature = 0, max_tokens = 256 })
            ```

            Ask the model.

            ```lua
            models.infer(prose)
            ```",
        )
        .prose(
            "Next",
            "
            Ask again.

            ```lua
            return models.infer(prose)
            ```",
        );
    let trace = Scenario::new(source.build())
        .unprepared()
        .context(bind_role("analyst", test_model("analyst")))
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "hello again");

    let requests = gateway.requests();
    assert_eq!(requests.len(), 2, "one round per section: {requests:?}");
    let selected = &requests[0];
    assert_eq!(selected.options.model(), "analyst");
    assert_eq!(selected.options.thinking(), Some(false));
    assert_eq!(
        selected
            .options
            .temperature()
            .map(crate::model::Temperature::get),
        Some(0.0)
    );
    assert_eq!(
        selected.options.max_tokens().map(NonZeroU32::get),
        Some(256)
    );
    let defaulted = &requests[1];
    assert_eq!(defaulted.options.model(), "analyst");
    assert_eq!(defaulted.options.thinking(), Some(false));
    assert!(
        defaulted.options.temperature().is_none() && defaulted.options.max_tokens().is_none(),
        "a section on the prompt-wide default sends neither option: {defaulted:?}"
    );
}

/// One section that infers its prose, `Say something.`, then runs `epilog`.
fn say_something(epilog: &str) -> String {
    let body = format!("Say something.\n\n```lua\n{epilog}\n```");
    with_writer(PromptSource::new())
        .prose("Only", &body)
        .build()
}

#[test]
fn an_explicit_client_is_used_instead_of_the_environment() {
    // The scenario's scripted model is what a caller configured from a
    // file passes; nothing here reads `PROMPTFORGE_*`, and the run still
    // reaches a gateway and reports its model turn.
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let trace = Scenario::new(say_something("return models.infer(prose)"))
        .bound()
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "hello from the mock");

    assert_eq!(
        run_events(&trace),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string(),),
            ("Only".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string()),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[test]
fn epilog_runs_after_prose_and_can_return() {
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let source = say_something(
        "local text = models.infer(prose)\n\
         assert(text == 'hello from the mock')\n\
         store.write('epilog-ran.txt', 'yes')\n\
         return 'epilog result'",
    );
    let prompt = parse_ok(&source);
    let entry = promptforge_parser::detail::entry(&prompt).expect("fixture has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_some());

    let trace = Scenario::new(source).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "epilog result");
    assert_eq!(trace.read("epilog-ran.txt").unwrap(), "yes");
    assert_eq!(
        run_events(&trace),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string()),
            ("Only".to_string(), detail::VFS_WRITE_SUCCEEDED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

/// The `Only` section that offers `web/search` before prose no model may
/// read.
const OFFER_WEB_SEARCH: &str = "
    ```lua
    tools.offer('web/search')
    ```

    This prose must not reach a model.";

#[test]
fn offer_over_an_empty_catalog_fails_the_run_loudly() {
    // A prompt with no H1 Lua goes through the same validated VM over an
    // empty offering, so the id is rejected.
    let source = PromptSource::new().prose("Only", OFFER_WEB_SEARCH);
    let trace = Scenario::new(source.build()).run();
    let error = trace.error();
    assert!(
        error
            .to_string()
            .contains("tools.offer: \"web/search\" is not a catalog tool in this run"),
        "the error must report the missing tool: {error}"
    );
}

#[test]
fn offer_after_an_h1_helper_fails_the_run_loudly() {
    // A prompt whose H1 Lua declares nothing runs over the same empty
    // offering, so tools.offer in a prologue is rejected the same way.
    let source = PromptSource::new()
        .h1("function helper() return 'no declarations' end")
        .prose("Only", OFFER_WEB_SEARCH);
    let trace = Scenario::new(source.build()).run();
    let error = trace.error();
    assert!(
        error
            .to_string()
            .contains("tools.offer: \"web/search\" is not a catalog tool in this run"),
        "the error must report the missing tool: {error}"
    );
}

#[test]
fn prologue_return_skips_model_and_epilog() {
    let source = PromptSource::new().prose(
        "Only",
        "
        ```lua
        return 'early'
        ```

        This prose must not reach a model.

        ```lua
        store.write('epilog-ran.txt', 'yes')
        return 'late'
        ```",
    );
    let trace = Scenario::new(source.build()).run();

    assert_eq!(trace.text(), "early");
    assert!(trace.read("epilog-ran.txt").is_err());
}

#[test]
fn a_required_helper_survives_prologue_model_and_epilog() {
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let source = with_writer(PromptSource::new())
        .prose(
            "Only",
            "
            ```lua
            require '## helpers'
            var.question = decorate(args)
            ```

            Ask using {{ var.question }}.

            ```lua
            return decorate(models.infer(prose))
            ```",
        )
        .section(
            "helpers",
            "function decorate(value) return '<' .. value .. '>' end",
        );
    let trace = Scenario::new(source.build())
        .args("input")
        .bound()
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "<hello from the mock>");
    assert_eq!(
        run_events(&trace),
        [
            ("Test prompt".to_owned(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_owned(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_owned(), detail::SECTION_STARTED.to_string()),
            ("Only".to_owned(), detail::LUA_CHUNK_STARTED.to_string()),
            (
                "Only".to_owned(),
                "Lua library load started: ## helpers".to_owned(),
            ),
            (
                "Only".to_owned(),
                "Lua library load succeeded: ## helpers".to_owned(),
            ),
            ("Only".to_owned(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_owned(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_owned(), detail::MODEL_TURN_COMPLETED.to_string(),),
            ("Only".to_owned(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_owned(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_owned(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_owned(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[test]
fn empty_prose_skips_model_but_runs_epilog_with_nil_reply() {
    let source = PromptSource::new().prose(
        "Only",
        "
        ```lua
        var.phase = 'prologue'
        ```

        ```lua
        if reply ~= nil then error('empty prose must not bind a reply') end
        return var.phase .. '-epilog'
        ```",
    );

    assert_eq!(
        Scenario::new(source.build()).run().text(),
        "prologue-epilog"
    );
}

#[test]
fn whitespace_only_prose_skips_model_without_binding() {
    // The source builder trims every line, which would empty the
    // whitespace-only prose under test, so the source is literal.
    let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\n-- prologue\n```\n\n   \n\t\n\n\
```lua\nif reply ~= nil then error('whitespace prose must not bind a reply') end\nreturn 'ok'\n```\n";
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn model_required_when_infer_has_no_binding() {
    // Prose itself never requires a model; only an explicit `models.infer`
    // of it does.
    let source = PromptSource::new().prose(
        "Only",
        "
        Ask the model.

        ```lua
        return models.infer(prose)
        ```",
    );
    let trace = Scenario::new(source.build()).run();
    let error = trace.error();
    assert_eq!(
        error.kind(),
        RunErrorKind::Binding,
        "expected ModelRequired, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model binding required for section Only"),
        "error must name the section: {error}"
    );
}

#[path = "model_and_reply-globals.rs"]
mod globals;
#[path = "model_and_reply-handles.rs"]
mod handles;
