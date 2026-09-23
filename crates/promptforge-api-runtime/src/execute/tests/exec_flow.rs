//! Tests for section walk control flow: `call`, `jump`, `fanout`, `list_from_section`, and `var`.

use super::run;
use super::*;
use promptforge_parser::test_support::synthetic_section;

/// The frontmatter every flow test shares, fused into the prompt literal at
/// compile time so a test states only its sections.
macro_rules! flow_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
            $body
        )
    };
}

/// Alternating lua/prose blocks run in order; each Lua block that wants the
/// model reads its own pending prose buffer into an explicit infer, and the
/// run result is the final Lua return.
#[tokio::test]
async fn section_with_alternating_blocks_executes_in_order() {
    let gateway = ScriptedGateway::start(vec![resp_text("reply-1"), resp_text("reply-2")]).await;
    let addr = gateway.addr();
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\nstore.append('order.txt', 'lua1\\n')\n```\n\n\
First ask.\n\n\
```lua\nstore.append('order.txt', 'lua2:' .. models.infer(prose) .. '\\n')\n```\n\n\
Final ask.\n\n\
```lua\nstore.append('order.txt', 'lua3\\n')\nreturn models.infer(prose)\n```\n"
    );
    let store = TestStore::new();
    let out = run(&bound_for_model(md), "", &[], &store, gatewayed(addr))
        .await
        .expect("alternating blocks must execute");

    assert_eq!(out, "reply-2");
    assert_eq!(gateway.call_count(), 2);
    assert_eq!(
        store.read("order.txt").expect("order log"),
        "lua1\nlua2:reply-1\nlua3\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_runs_named_section_as_subroutine() {
    let gateway = ScriptedGateway::start(vec![resp_text("research-reply")]).await;
    let addr = gateway.addr();

    // The subroutine sits after its run-ending caller: a contained chain
    // falls through like any walk, so a subroutine placed before later
    // sections would pull them into the chain.
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local by_name = call('## Research')\n\
assert(by_name == 'research-reply')\n\
assert(store.read('evidence.md') == 'research-reply')\n\
return by_name\n\
```\n\n\
## Research\n\n\
Research {{ args }}.\n\n\
```lua\n\
local answer = models.infer(prose)\n\
store.write('evidence.md', answer)\n\
return answer\n\
```\n"
    );
    let store = TestStore::new();
    let out = run(&bound_for_model(md), "topic", &[], &store, gatewayed(addr))
        .await
        .expect("call must run named section as subroutine");
    assert_eq!(out, "research-reply");
}

/// The `with_args` fork scopes a call's input over its whole chain:
/// a no-input `call` nested inside the chain defaults to the chain's
/// args, not the run's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_call_without_input_inherits_the_chains_args() {
    let gateway = ScriptedGateway::start(vec![resp_text("inner-reply")]).await;
    let addr = gateway.addr();
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local r = call('## Sub', 'chain-args')\n\
assert(r == 'inner-reply')\n\
return r\n\
```\n\n\
## Sub\n\n\
```lua\nreturn call('## Inner')\n```\n\n\
## Inner\n\n\
Args: {{ args }}\n\n\
```lua\nreturn models.infer(prose)\n```\n"
    );
    let store = TestStore::new();
    let out = run(
        &bound_for_model(md),
        "run-args",
        &[],
        &store,
        gatewayed(addr),
    )
    .await
    .expect("the nested call must inherit the chain's args");
    assert_eq!(out, "inner-reply");
    let body = gateway
        .last_request()
        .expect("the inner section's infer must reach the gateway");
    let text = body.to_string();
    assert!(
        text.contains("chain-args"),
        "the nested no-input call substitutes the chain's args: {text}"
    );
    assert!(
        !text.contains("run-args"),
        "the run's args must not leak into the chain: {text}"
    );
}

/// Two visible sections sharing one `(level, name)` address error loudly as
/// ambiguous rather than silently resolving to the first.
#[test]
fn list_from_section_ambiguous_error_is_loud() {
    let visible = vec![
        synthetic_section("Dup", 3, Vec::new(), vec!["x".to_string()]),
        synthetic_section("Dup", 3, Vec::new(), vec!["x".to_string()]),
    ];
    let error = super::super::engine::list_items_from_visible("### Dup", &visible)
        .expect_err("two visible sections with one address must be ambiguous");
    let rendered = error.to_string();
    assert!(rendered.contains("ambiguous"), "error was: {rendered}");
}

/// Two top-level sections sharing one name error loudly as ambiguous rather
/// than silently resolving to the first (the retired `resolve_h2_section`
/// first-match behavior).
#[test]
fn duplicate_top_level_section_names_error_loudly() {
    let sections = vec![
        synthetic_section("Main", 2, Vec::new(), Vec::new()),
        synthetic_section("Dup", 2, Vec::new(), Vec::new()),
        synthetic_section("Dup", 2, Vec::new(), Vec::new()),
    ];
    let error = super::super::engine::resolve_jump_target("## Dup", &sections, &sections[0])
        .expect_err("two visible sections with one name must be ambiguous");
    let rendered = error.to_string();
    assert!(rendered.contains("ambiguous"), "error was: {rendered}");
}

/// The scaffold the fanout-arm capability tests share: a `## Parent` that
/// fans `### Worker` out over one `alpha` member and returns the first arm's
/// text. The worker body and its sibling sections follow.
const ARM_FANOUT_PARENT: &str = flow_prompt!(
    "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha'})\n\
return r[1].text\n\
```\n\n"
);

/// `models.infer(handle, ...)` works inside an arm: the arm installs the infer hook, so a
/// worker's Lua can call the model directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_works_inside_an_arm() {
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.infer(models.get('writer'), 'ping about ' .. item)\n\
```\n",
    ]
    .concat();
    let out = run(
        &bound_for_model(&md),
        "",
        &[],
        &TestStore::new(),
        gatewayed(addr),
    )
    .await
    .expect("handle infer inside an arm must run");
    assert_eq!(out, "pong");

    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(body["model"], "claude-sonnet-4-6");
    let messages = body["messages"].as_array().expect("messages array");
    let content = messages.last().expect("a user turn")["content"]
        .as_str()
        .expect("content string");
    assert!(
        content.contains("ping about alpha"),
        "the arm's item must reach the infer prompt: {content}"
    );
}

/// `models.infer(handle, ...)` inside an arm handed no client surfaces the lazy-creation
/// error through the infer hook.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_without_a_client_surfaces_the_disabled_gateway() {
    // A host without a client performs every `Chat` against the disabled
    // gateway: the round fails with that error and nothing reaches the
    // network, however the process environment is configured.
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.infer(models.get('writer'), 'ping about ' .. item)\n\
```\n",
    ]
    .concat();
    let error = run(&bound_for_model(&md), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("handle infer in an arm with no client must surface the disabled gateway");
    let rendered = error.to_string();
    assert!(
        rendered.contains("gateway access is disabled"),
        "the infer hook must surface the disabled-gateway completion error: {rendered}"
    );
}

/// An unknown model alias errors loudly inside an arm, same as on the walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_with_an_unknown_alias_errors_loudly() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.infer(models.get('ghost'), 'ping')\n\
```\n",
    ]
    .concat();
    let error = run(&bound_for_model(&md), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("an unknown model alias inside an arm must fail loudly");
    let rendered = error.to_string();
    assert!(
        rendered.contains("models.get alias \"ghost\" is not a bound model role"),
        "the unknown alias must be named: {rendered}"
    );
}

#[test]
fn advance_turn_saturates_and_never_wraps_the_stored_counter() {
    // FANOUT-008: the shared turn counter must saturate at u32::MAX rather than
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
