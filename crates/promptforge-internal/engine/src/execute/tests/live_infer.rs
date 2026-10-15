//! Tests for live `models.infer` against a scripted gateway, including H1 chunks and required libraries.

use promptforge_parser::test_support::{PromptSource, parse_ok};

use super::*;
use crate::cancel::CancelHandle;

/// The prompt declaring the `writer` role, with no default parked.
fn writer_role() -> PromptSource {
    PromptSource::new().front("models: { writer: {} }")
}

/// The prompt whose H1 infers once through the `writer` handle into
/// `var.answer`, which its `Result` section returns.
fn h1_handle_infer() -> String {
    writer_role()
        .h1("
            local writer = models.default('writer')
            var.answer = writer:infer('answer once')")
        .section("Result", "return var.answer")
        .build()
}

#[test]
fn live_h1_infer_runs_once() {
    let gateway = ScriptedChat::new(vec![text("h1 answer")]);
    let trace = Scenario::new(h1_handle_infer())
        .bound()
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "h1 answer");
    assert_eq!(gateway.call_count(), 1);
}

#[test]
fn the_callers_client_serves_a_run_the_context_never_names() {
    // The context is the Engine's input; the scripted model lives on the
    // scenario, which answers the run's completions with it. Nothing about
    // the gateway crosses the Engine's boundary.
    let gateway = ScriptedChat::new(vec![text("canned answer")]);
    let trace = Scenario::new(h1_handle_infer())
        .bound()
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "canned answer");
    assert_eq!(
        gateway.call_count(),
        1,
        "the completion must have gone to the scenario's model"
    );
}

#[test]
fn unread_h1_prose_stays_inert_and_explicit_infer_requires_a_model() {
    // H1 prose does not drive inference: an unread buffer - even one
    // whose substitution would fail or stay empty - discards at the pass's
    // end without requiring a model. Only an explicit `models.infer` of the
    // prose requires a binding.
    let unread = writer_role()
        .h1("var.omit = ''")
        .h1_prose("{{ var.omit }}")
        .section("Result", "return 'ok'");
    let trace = Scenario::new(unread.build()).run();
    assert_eq!(
        trace.text(),
        "ok",
        "unread H1 prose must not require a model"
    );

    let reading = writer_role()
        .h1_prose("ask")
        .h1("return models.infer(prose)");
    let trace = Scenario::new(reading.build()).run();
    let error = trace.error();
    assert_eq!(
        error.kind(),
        RunErrorKind::Binding,
        "expected ModelRequired, got {error}"
    );
    assert!(
        error.to_string().contains("model binding required"),
        "expected ModelRequired, got {error}"
    );
}

#[test]
fn a_library_function_resolves_engine_globals_when_called() {
    let source = writer_role()
        .section("Result", "require '## helpers'\nreturn read_args()")
        .section("helpers", "function read_args() return args end");
    let trace = Scenario::new(source.build())
        .args("later canned value")
        .bound()
        .run();

    assert_eq!(trace.text(), "later canned value");
}

#[test]
fn a_library_calls_engine_globals_at_load_time() {
    // A library loads into the requiring section's environment with every
    // Engine global installed, so its top-level code may use `store`,
    // `log`, and `args` at load.
    let source = writer_role()
        .section(
            "Result",
            "require '## loader'\nreturn store.read('loaded.txt')",
        )
        .section(
            "loader",
            "
            store.write('loaded.txt', args)
            log('library loaded')",
        )
        .build();
    let (_, requirements) = test_context(EXECUTION)
        .model(test_model(TEST_MODEL))
        .prepare(&parse_ok(&source));
    assert!(
        requirements.is_satisfied(),
        "the fixture declares nothing: {requirements:?}"
    );
    let trace = Scenario::new(source).args("load-time args").bound().run();

    assert_eq!(trace.text(), "load-time args");
    assert_eq!(
        trace
            .read("loaded.txt")
            .expect("the load-time write persists"),
        "load-time args"
    );
}

#[test]
fn tool_objects_and_roles_reach_section_call_and_fanout_vms() {
    // The offering arrives from the catalog: the Plugin installs the tool,
    // and every section VM reads its tool object by id - H1 never runs a
    // bind.
    let source = writer_role()
        .front("plugins: [tools]")
        .section(
            "Parent",
            "
            require '## bindings'
            local direct = binding_names()
            local called = call('## Called')
            local arms = tasks.fanout('### Worker', prompt.list('### Items'))
            return direct .. '|' .. called .. '|' .. table.concat(arms, ',')",
        )
        .section(
            "### Worker",
            "require '## bindings'\nreturn binding_names() .. ':' .. item",
        )
        .prose("### Items", "- one\n- two")
        .section("Called", "require '## bindings'\nreturn binding_names()")
        .section(
            "bindings",
            "function binding_names() return tools.get('tools/echo').id .. ':' .. models.get('writer').name end",
        );
    // The fixture tool's descriptor goes into the catalog and its
    // implementation answers the run's calls; the run offers every
    // catalog tool.
    let trace = Scenario::new(source.build())
        .bound()
        .tool(echo_tool())
        .run();

    assert_eq!(
        trace.text(),
        "tools/echo:writer|tools/echo:writer|tools/echo:writer:one,tools/echo:writer:two"
    );
}

#[test]
fn live_h1_models_infer_resolves_the_default_model_without_touching_sys() {
    // The live H1 `models.infer` resolves the current model from the
    // producer's bindings-so-far and runs the one infer shape: a single
    // tool-free round on a fresh conversation that leaves `sys` untouched.
    let gateway = ScriptedChat::new(vec![text("h1 answer")]);
    let source = writer_role()
        .h1("
            models.default('writer')
            var.answer = models.infer('answer once')
            var.sys_untouched = not pcall(function() return sys.reply_finish_reason end)")
        .section(
            "Result",
            "return var.answer .. ':' .. tostring(var.sys_untouched)",
        );
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "h1 answer:true");
    assert_eq!(gateway.call_count(), 1);
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(
        body.options.model(),
        "claude-sonnet-4-6",
        "models.infer must use the section's current model"
    );
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
fn nested_lua_infer_emits_a_model_turn_observation() {
    // A nested Lua infer must surface its model-turn observation to the
    // run's observer, proving owned-observer propagation reaches the nested
    // inference path.
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let source = writer_role()
        .h1("
            local writer = models.default('writer')
            var.answer = writer:infer('ping')")
        .section("Result", "return var.answer");
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "pong");
    let details: Vec<String> = trace
        .records()
        .into_iter()
        .map(|record| record.detail)
        .collect();
    let model_turns = details
        .iter()
        .filter(|d| d.as_str() == "Model turn completed")
        .count();
    assert_eq!(
        model_turns, 1,
        "the nested infer drives exactly one model round trip: {details:?}"
    );
    assert!(
        details.iter().all(|d| d.as_str() != "Tool call succeeded"),
        "infer advertises no tools, so no tool call can be observed: {details:?}"
    );
}

#[test]
fn cancelled_nested_infer_does_not_report_model_turn_failed() {
    let gateway = ScriptedChat::new(vec![delayed(
        std::time::Duration::from_secs(30),
        text("too late"),
    )]);
    let source = writer_role().h1("
        local writer = models.default('writer')
        return writer:infer('must cancel')");
    let cancel = CancelHandle::new();
    let canceller = cancel.clone();
    let mut driving = Scenario::new(source.build())
        .bound()
        .chat(&gateway)
        .context(move |ctx| ctx.cancel(canceller))
        .hold(|effect| matches!(effect, Effect::Chat { .. }))
        .start();
    while driving.step() {}
    cancel.cancel();
    let trace = driving.finish();
    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "cancelling an in-flight infer must interrupt the run: {:?}",
        trace.result()
    );
    assert!(
        matches!(
            trace.effects(),
            [(EffectRecord::Chat { .. }, AnswerRecord::Dropped)]
        ),
        "the cancellation must occur after infer reached the caller: {:?}",
        trace.effects()
    );
    let records = trace.records();
    assert!(
        records
            .iter()
            .all(|record| record.detail != detail::MODEL_TURN_FAILED.to_string()),
        "cancellation must not report a model failure: {records:?}"
    );
}

#[test]
fn handle_infer_tool_call_violation_uses_entry_point_neutral_wording() {
    let gateway = ScriptedChat::new(vec![calls(&[("call_1", "ghost", "{}")])]);
    let source = writer_role().h1("
        local writer = models.default('writer')
        return writer:infer('answer without tools')");
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("model inference received tool calls but no tools were advertised"),
        "the violation must use neutral wording: {rendered}"
    );
    assert!(
        !rendered.contains("models.infer received"),
        "handle-form infer must not be misreported as models.infer: {rendered}"
    );
}

#[test]
fn live_h1_prose_infers_explicitly_and_var_accumulates_into_the_walk() {
    // The live H1 pass reads its pending buffer only through an explicit
    // infer, and `var` writes accumulate across the pass into the walk.
    let gateway = ScriptedChat::new(vec![text("final answer")]);
    let source = writer_role()
        .h1("
            models.default('writer')
            var.executions = (var.executions or 0) + 1")
        .h1_prose("Ask for one round.")
        .h1("
            var.first = models.infer(prose)
            var.executions = var.executions + 1")
        .section("Result", "return var.first .. ':' .. var.executions");
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "final answer:2");
    assert_eq!(gateway.call_count(), 1);
}

#[test]
fn h1_and_h2_prose_each_infer_explicitly_in_source_order() {
    // The live H1 pass and the H2 section each read their own pending
    // buffer into an explicit infer: two completions, in source order.
    let gateway = ScriptedChat::new(vec![text("h1 reply"), text("h2 reply")]);
    let source = writer_role()
        .h1("models.default('writer')")
        .h1_prose("h1 prose turn")
        .h1("var.h1 = models.infer(prose)")
        .prose(
            "Section Two",
            "
            h2 prose turn

            ```lua
            return models.infer(prose)
            ```",
        );
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "h2 reply");
    assert_eq!(
        gateway.call_count(),
        2,
        "the H1 prose and the H2 prose each drive exactly one completion"
    );
    let requests = gateway.requests();
    let first_prose = requests[0].messages[0].content();
    let second_prose = requests[1].messages[0].content();
    assert!(
        first_prose.contains("h1 prose turn"),
        "the first completion is the H1 prose: {first_prose}"
    );
    assert!(
        second_prose.contains("h2 prose turn"),
        "the second completion is the H2 prose: {second_prose}"
    );
}

#[test]
fn live_h1_chunk_takes_root_entry_zero_and_the_first_walked_section_takes_root_entry_one() {
    // The H1 pass is the root chain's entry 0, so the first walked section
    // takes entry 1 of the same chain.
    let source = writer_role()
        .h1("assert(sys.id == '0.0', 'the live H1 chunk takes the root chain entry 0')")
        .section(
            "Result",
            "
            assert(sys.id == '0.1', 'the first walked section takes entry 1')
            return 'ok'",
        );
    let trace = Scenario::new(source.build()).bound().run();

    assert_eq!(trace.text(), "ok");
}
