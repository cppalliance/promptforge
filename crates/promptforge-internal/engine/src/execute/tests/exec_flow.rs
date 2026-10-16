//! Tests for section walk control flow: `call`, `jump`, `fanout`, the `prompt` reads, and `var`.

use promptforge_parser::test_support::{PromptSource, synthetic_section};

use super::context::{arm_fanout, with_writer};
use super::*;

/// Alternating lua/prose blocks run in order; each Lua block that wants the
/// model reads its own pending prose buffer into an explicit infer, and the
/// run result is the final Lua return.
#[test]
fn section_with_alternating_blocks_executes_in_order() {
    let gateway = ScriptedChat::new(vec![text("reply-1"), text("reply-2")]);
    let source = with_writer(PromptSource::new()).prose(
        "Only",
        r"
        ```lua
        store.append('order.txt', 'lua1\n')
        ```

        First ask.

        ```lua
        store.append('order.txt', 'lua2:' .. models.infer(prose) .. '\n')
        ```

        Final ask.

        ```lua
        store.append('order.txt', 'lua3\n')
        return models.infer(prose)
        ```",
    );
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();

    assert_eq!(trace.text(), "reply-2");
    assert_eq!(gateway.call_count(), 2);
    assert_eq!(
        trace.read("order.txt").expect("order log"),
        "lua1\nlua2:reply-1\nlua3\n"
    );
}

#[test]
fn call_runs_named_section_as_subroutine() {
    let gateway = ScriptedChat::new(vec![text("research-reply")]);

    // The subroutine sits after its run-ending caller: a contained chain
    // falls through like any walk, so a subroutine placed before later
    // sections would pull them into the chain.
    let source = with_writer(PromptSource::new())
        .section(
            "Main",
            "
            local by_name = call('## Research')
            assert(by_name == 'research-reply')
            assert(store.read('evidence.md') == 'research-reply')
            return by_name",
        )
        .prose(
            "Research",
            "
            Research {{ args }}.

            ```lua
            local answer = models.infer(prose)
            store.write('evidence.md', answer)
            return answer
            ```",
        );
    let trace = Scenario::new(source.build())
        .args("topic")
        .bound()
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "research-reply");
}

/// The `with_args` fork scopes a call's input over its whole chain:
/// a no-input `call` nested inside the chain defaults to the chain's
/// args, not the run's.
#[test]
fn nested_call_without_input_inherits_the_chains_args() {
    let gateway = ScriptedChat::new(vec![text("inner-reply")]);
    let source = with_writer(PromptSource::new())
        .section(
            "Main",
            "
            local r = call('## Sub', 'chain-args')
            assert(r == 'inner-reply')
            return r",
        )
        .section("Sub", "return call('## Inner')")
        .prose(
            "Inner",
            "
            Args: {{ args }}

            ```lua
            return models.infer(prose)
            ```",
        );
    let trace = Scenario::new(source.build())
        .args("run-args")
        .bound()
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "inner-reply");
    let body = gateway
        .last_request()
        .expect("the inner section's infer must reach the gateway");
    let text = body.messages_json().to_string();
    assert!(
        text.contains("chain-args"),
        "the nested no-input call substitutes the chain's args: {text}"
    );
    assert!(
        !text.contains("run-args"),
        "the run's args must not leak into the chain: {text}"
    );
}

/// Two top-level sections sharing one name error loudly as ambiguous rather
/// than silently resolving to the first.
#[test]
fn duplicate_top_level_section_names_error_loudly() {
    let sections = vec![
        synthetic_section("Main", 2, Vec::new()),
        synthetic_section("Dup", 2, Vec::new()),
        synthetic_section("Dup", 2, Vec::new()),
    ];
    let error = crate::execute::walk_target::resolve_jump_target("## Dup", &sections, 0)
        .expect_err("two visible sections with one name must be ambiguous");
    let rendered = error.to_string();
    assert!(rendered.contains("ambiguous"), "error was: {rendered}");
}

/// [`arm_fanout`] under the `writer` role, with a `### Worker` whose Lua is
/// `worker`.
fn writer_arm(worker: &str) -> String {
    with_writer(arm_fanout())
        .section("### Worker", worker)
        .build()
}

/// The worker Lua that asks the model through the `writer` handle.
const HANDLE_INFER: &str = "return models.get('writer'):infer('ping about ' .. item)";

/// `handle:infer(...)` works inside an arm, so a worker's Lua can call the model
/// directly.
#[test]
fn fanout_arm_model_infer_works_inside_an_arm() {
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let trace = Scenario::new(writer_arm(HANDLE_INFER))
        .bound()
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "pong");

    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(body.options.model(), "claude-sonnet-4-6");
    let messages = body.messages_json();
    let content = messages
        .as_array()
        .expect("messages array")
        .last()
        .expect("a user turn")["content"]
        .as_str()
        .expect("content string");
    assert!(
        content.contains("ping about alpha"),
        "the arm's item must reach the infer prompt: {content}"
    );
}

/// `handle:infer(...)` inside an arm handed no client surfaces the lazy-creation
/// error.
#[test]
fn fanout_arm_model_infer_without_a_client_surfaces_the_disabled_gateway() {
    // A scenario without a model answers every `Chat` with the disabled
    // gateway's failure: the round fails with that error and nothing
    // reaches the network, however the process environment is configured.
    let trace = Scenario::new(writer_arm(HANDLE_INFER)).bound().run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("model access is turned off or not configured"),
        "an arm's models.infer must surface the disabled-gateway completion error: {rendered}"
    );
}

/// An unknown model alias errors loudly inside an arm, same as on the walk.
#[test]
fn fanout_arm_model_infer_with_an_unknown_alias_errors_loudly() {
    let trace = Scenario::new(writer_arm("return models.get('ghost'):infer('ping')"))
        .bound()
        .run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("models.get alias \"ghost\" is not a bound model role"),
        "the unknown alias must be named: {rendered}"
    );
}

#[test]
fn advance_turn_saturates_and_never_wraps_the_stored_counter() {
    // The shared turn counter must saturate at u32::MAX rather than
    // wrapping through fetch_add and reusing a turn index.
    let turns = AtomicU32::new(0);
    assert_eq!(advance_turn(&turns), 1);
    assert_eq!(advance_turn(&turns), 2);
    assert_eq!(turns.load(Ordering::Relaxed), 2);

    // At the boundary, both the presented value and the stored value saturate.
    let maxed = AtomicU32::new(u32::MAX);
    assert_eq!(advance_turn(&maxed), u32::MAX);
    assert_eq!(
        maxed.load(Ordering::Relaxed),
        u32::MAX,
        "the stored counter must not wrap to zero"
    );

    let near = AtomicU32::new(u32::MAX - 1);
    assert_eq!(advance_turn(&near), u32::MAX);
    assert_eq!(advance_turn(&near), u32::MAX);
    assert_eq!(near.load(Ordering::Relaxed), u32::MAX);
}
