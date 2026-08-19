use super::super::*;
use super::run;
use super::*;

/// Alternating lua/prose blocks run in order; non-final prose is single-shot,
/// final prose loops, and trailing lua sees the last reply.
#[tokio::test]
async fn section_with_alternating_blocks_executes_in_order() {
    async fn completions(
        State(calls): State<Arc<AtomicU32>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        let n = calls.fetch_add(1, Ordering::Relaxed) + 1;
        Json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": format!("reply-{n}")
                }
            }]
        }))
    }

    let calls = Arc::new(AtomicU32::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\nstore.append('order.txt', 'lua1\\n')\n```\n\n\
First ask.\n\n\
```lua\nstore.append('order.txt', 'lua2\\n')\n```\n\n\
Final ask.\n\n\
```lua\nstore.append('order.txt', 'lua3\\n')\nreturn reply\n```\n";
    let store = StoreRef::memory();
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("alternating blocks must execute");

    assert_eq!(out, "reply-2");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        store.read("order.txt").expect("order log"),
        "lua1\nlua2\nlua3\n"
    );

    let section = parse(md).entry().expect("fixture has sections").clone();
    assert_eq!(section.blocks.len(), 5);
    assert!(matches!(
        &section.blocks[1],
        crate::parser::Block::Prose {
            loop_capable: false,
            ..
        }
    ));
    assert!(matches!(
        &section.blocks[3],
        crate::parser::Block::Prose {
            loop_capable: true,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_runs_named_section_as_subroutine() {
    async fn completions(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "research-reply"
                }
            }]
        }))
    }

    let router = Router::new().route("/v1/chat/completions", post(completions));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Research\n\n\
```lua\n\
local step = tasks['## Research']\n\
assert(step.name == 'Research')\n\
assert(step.has_prose == true)\n\
```\n\n\
Research {{ args }}.\n\n\
```lua\nstore.write('evidence.md', reply)\n```\n\n\
## Main\n\n\
```lua\n\
local research = tasks['## Research']\n\
assert(research.name == 'Research')\n\
assert(research.has_prose == true)\n\
local by_name = execute('## Research')\n\
local by_obj = execute(research)\n\
assert(by_name == 'research-reply')\n\
assert(by_obj == 'research-reply')\n\
assert(store.read('evidence.md') == 'research-reply')\n\
return by_name\n\
```\n";
    let store = StoreRef::memory();
    let out = run(
        &bound_for_model(md),
        "topic",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("execute must run named section as subroutine");
    assert_eq!(out, "research-reply");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_transfers_control_and_preserves_reply() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Check\n\n\
```lua\n\
store.write('seen.txt', 'check')\n\
local help = tasks['## Help']\n\
assert(help.name == 'Help')\n\
assert(help.has_prose == false)\n\
jump(help)\n\
store.write('seen.txt', 'should-not-run')\n\
```\n\n\
## Accept\n\n\
```lua\nreturn 'accepted'\n```\n\n\
## Help\n\n\
```lua\n\
assert(reply == nil, 'no prior reply because no prose ran before the jump')\n\
return 'helped:' .. store.read('seen.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("jump must transfer control");
    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

/// The subroutine walk rejects `jump()` (unlike the top-level walk, which
/// follows it). Locks the `JumpPolicy::Reject` divergence folded into the
/// unified section engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_preserves_reply_from_prior_section() {
    let gateway = ScriptedGateway::start(vec![resp_text("model-said-this")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Source\n\n\
Ask something.\n\n\
```lua\njump('## Target')\n```\n\n\
## Target\n\n\
```lua\n\
assert(reply ~= nil, 'jump must preserve the prior reply')\n\
return reply\n\
```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("jump must preserve the reply from the prior section");
    assert_eq!(out, "model-said-this");
}

/// The subroutine walk rejects `jump()` (unlike the top-level walk, which
/// follows it). Locks the `JumpPolicy::Reject` divergence folded into the
/// unified section engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_execute_is_rejected() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\nreturn execute('## Sub')\n```\n\n\
## Sub\n\n\
```lua\n\
local m = tasks['## Main']\n\
jump(m)\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("jump inside execute must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("not allowed inside execute"),
        "expected a jump-rejection error, got: {rendered}"
    );
}

/// Nested `execute()` is capped at [`MAX_EXECUTE_DEPTH`]. Locks the
/// `execute_depth` divergence threaded through the unified engine (the
/// top-level walk always enters at depth 0; the subroutine carries its depth).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_recursion_is_capped() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Loop\n\n\
```lua\nreturn execute('## Loop')\n```\n";
    let error = run_offline(md)
        .await
        .expect_err("unbounded execute recursion must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("recursion exceeded cap"),
        "expected a recursion-cap error, got: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_returns_structured_results() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
assert(r[1].text == 'alpha-1')\n\
assert(r[1].ok == true)\n\
assert(r[1].item == 'alpha')\n\
assert(r[1].exhausted == false)\n\
assert(r[2].text == 'beta-2')\n\
assert(r[2].ok == true)\n\
assert(r[2].item == 'beta')\n\
assert(r[2].exhausted == false)\n\
assert(tostring(r[1]) == r[1].text)\n\
assert(table.concat(r, ',') == 'alpha-1,beta-2')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item .. '-' .. sys.taskid\n```\n\n\
Do work.\n\n\
### Items\n\n\
- alpha\n\
- beta\n";
    let out = run(&fixture(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect("fanout must return structured results");
    assert_eq!(out, "ok");
}

/// An off-walk section is never visited by the walk: no observation, no
/// execution. It stays in the section tree, so `sys.section_count` counts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_section_is_never_visited_by_the_walk() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Hidden\n\n\
---\n\n\
```lua\nerror('hidden must not run')\n```\n\n\
## Main\n\n\
```lua\n\
assert(sys.section_count == 2)\n\
return 'main-ran'\n\
```\n";
    let (result, records) = run_recorded(md).await;
    assert_eq!(
        result.expect("the walk must skip the off-walk section"),
        "main-ran"
    );
    assert!(
        records.iter().all(|(_, section, _)| section != "Hidden"),
        "an off-walk section must produce no observations: {records:?}"
    );
}

/// The canonical shape: A executes B and C (both off-walk), D unmarked - the
/// walk visits A then D, and the off-walk sections' content below the marker
/// runs when addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_sections_run_only_when_addressed() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local rb = execute('## B')\n\
local rc = execute('## C')\n\
store.write('order.txt', rb .. ',' .. rc)\n\
```\n\n\
## B\n\n\
---\n\n\
```lua\nreturn 'b-ran'\n```\n\n\
## C\n\n\
---\n\n\
```lua\nreturn 'c-ran'\n```\n\n\
## D\n\n\
```lua\nreturn 'd-ran:' .. store.read('order.txt')\n```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("off-walk sections must run when addressed");
    // A walked B or C would end the run early with its own scalar return.
    assert_eq!(out, "d-ran:b-ran,c-ran");
}

/// A jump addresses an off-walk section directly, so it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_off_walk_section_runs_it() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\njump('## B')\n```\n\n\
## B\n\n\
---\n\n\
```lua\nreturn 'b-ran'\n```\n\n\
## C\n\n\
```lua\nreturn 'c-ran'\n```\n";
    let out = run_offline(md)
        .await
        .expect("a jump to an off-walk section must run it");
    assert_eq!(out, "b-ran");
}

/// A jump runs its off-walk target, but the fall-through that follows is an
/// ordinary walk step: the next off-walk sibling is skipped again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fall_through_after_a_jumped_off_walk_section_skips_again() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\njump('## B')\n```\n\n\
## B\n\n\
---\n\n\
```lua\nlocal b = 1\n```\n\n\
## C\n\n\
---\n\n\
```lua\nreturn 'c-ran'\n```\n\n\
## D\n\n\
```lua\nreturn 'd-ran'\n```\n";
    let out = run_offline(md)
        .await
        .expect("fall-through after an addressed off-walk section must resume skipping");
    assert_eq!(out, "d-ran");
}

/// An off-walk child section still runs as a fanout worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_worker_runs_as_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
return r[1].text .. ',' .. r[2].text\n\
```\n\n\
### Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n\n\
### Items\n\n\
- alpha\n\
- beta\n";
    let out = run_offline(md)
        .await
        .expect("an off-walk worker must run as a fanout arm");
    assert_eq!(out, "alpha-done,beta-done");
}

/// `list_from_section` returns a sibling list section's pre-parsed bullet
/// items as a Lua array of strings, addressed by heading string or by a
/// Section object from the `tasks` table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_returns_bullet_items() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local items = list_from_section('## List')\n\
assert(type(items) == 'table')\n\
assert(#items == 2)\n\
assert(items[1] == 'alpha')\n\
assert(items[2] == 'beta')\n\
local by_handle = list_from_section(tasks['## List'])\n\
assert(#by_handle == 2 and by_handle[1] == 'alpha')\n\
return 'ok'\n\
```\n\n\
## List\n\n\
- alpha\n\
- beta\n";
    let out = run_offline(md)
        .await
        .expect("a sibling list section's bullet items must be returned");
    assert_eq!(out, "ok");
}

/// A numbered list section's items come back in order, markers stripped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_returns_numbered_items() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local items = list_from_section('## Nums')\n\
assert(#items == 3)\n\
assert(items[1] == 'one' and items[2] == 'two' and items[3] == 'three')\n\
return 'ok'\n\
```\n\n\
## Nums\n\n\
1. one\n\
2. two\n\
3. three\n";
    let out = run_offline(md)
        .await
        .expect("a numbered list section's items must be returned");
    assert_eq!(out, "ok");
}

/// The caller's direct children are in the visible set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_resolves_a_direct_child() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local items = list_from_section('### Sub')\n\
assert(#items == 2 and items[1] == 'x' and items[2] == 'y')\n\
return 'ok'\n\
```\n\n\
### Sub\n\n\
- x\n\
- y\n";
    let out = run_offline(md)
        .await
        .expect("a direct child list section must be visible");
    assert_eq!(out, "ok");
}

/// A sibling's child (niece), a child's child (grandchild), and the caller
/// itself are all outside the visible set: each resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_hides_nieces_grandchildren_and_the_caller() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local ok_niece = pcall(list_from_section, '### Niece')\n\
assert(not ok_niece, 'a niece is not visible')\n\
local ok_grand = pcall(list_from_section, '#### Grand')\n\
assert(not ok_grand, 'a grandchild is not visible')\n\
local ok_self = pcall(list_from_section, '## Main')\n\
assert(not ok_self, 'the caller itself is not visible')\n\
return 'ok'\n\
```\n\n\
### Kid\n\n\
- kid-item\n\n\
#### Grand\n\n\
- grand-item\n\n\
## Other\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
- niece-item\n";
    let out = run_offline(md)
        .await
        .expect("nieces, grandchildren, and the caller must all be invisible");
    assert_eq!(out, "ok");
}

/// The not-found error lists exactly the visible sections - siblings plus
/// direct children - so the error channel cannot leak the rest of the
/// document's structure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_not_found_lists_only_the_visible_sections() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
list_from_section('## Missing')\n\
```\n\n\
### Kid\n\n\
- kid-item\n\n\
#### Grand\n\n\
- grand-item\n\n\
## Sibling\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
- niece-item\n";
    let error = run_offline(md)
        .await
        .expect_err("an unknown heading must fail");
    let rendered = error.to_string();
    assert!(rendered.contains("not found"), "error was: {rendered}");
    assert!(
        rendered.contains("## Sibling"),
        "a sibling is visible: {rendered}"
    );
    assert!(
        rendered.contains("### Kid"),
        "a direct child is visible: {rendered}"
    );
    let (_, available) = rendered
        .split_once("available siblings:")
        .expect("the not-found error must list the visible sections");
    assert!(
        !available.contains("## Main"),
        "the caller is not listed: {rendered}"
    );
    assert!(
        !available.contains("Niece"),
        "a niece is not listed: {rendered}"
    );
    assert!(
        !available.contains("Grand"),
        "a grandchild is not listed: {rendered}"
    );
}

/// Two visible sections sharing one `(level, name)` address error loudly as
/// ambiguous rather than silently resolving to the first. Unreachable through
/// a real prompt (the parser forbids duplicate sibling names), so the
/// resolution helper is exercised directly with a synthetic visible set.
#[test]
fn list_from_section_ambiguous_error_is_loud() {
    fn list(name: &str) -> crate::parser::Section {
        crate::parser::Section {
            name: name.to_string(),
            level: 3,
            blocks: Vec::new(),
            children: Vec::new(),
            items: vec!["x".to_string()],
            off_walk: false,
        }
    }
    let visible = vec![list("Dup"), list("Dup")];
    let error = super::super::engine::list_items_from_visible("### Dup", &visible)
        .expect_err("two visible sections with one address must be ambiguous");
    let rendered = error.to_string();
    assert!(rendered.contains("ambiguous"), "error was: {rendered}");
}

/// Naming a prose section (no pre-parsed items) is the mistake the no-items
/// error exists to catch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_rejects_a_prose_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
list_from_section('## Prose')\n\
```\n\n\
## Prose\n\n\
Just prose, no list items here.\n";
    let error = run_offline(md)
        .await
        .expect_err("a prose section has no pre-parsed items");
    let rendered = error.to_string();
    assert!(
        rendered.contains("section `Prose` has no pre-parsed items"),
        "error was: {rendered}"
    );
}

/// Inside a fanout arm the global is a loud stub, same as execute/fanout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_is_not_available_inside_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
local items = list_from_section('### Items')\n\
return item\n\
```\n\n\
### Items\n\n\
- alpha\n";
    let error = run_offline(md)
        .await
        .expect_err("list_from_section inside an arm must fail loudly");
    let rendered = error.to_string();
    assert!(
        rendered.contains("list_from_section() is not available inside a fanout arm"),
        "error was: {rendered}"
    );
}

/// The H1 VM never gets the control globals, so `list_from_section` is nil
/// there and calling it is an ordinary Lua error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_is_absent_on_the_h1() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
```lua\n\
list_from_section('## Nope')\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("list_from_section must be absent on the H1");
    let rendered = error.to_string();
    assert!(
        rendered.contains("list_from_section"),
        "the nil-call error must name the global: {rendered}"
    );
}

/// The flagship composition: a sibling list section marked off-walk returns
/// its items through `list_from_section` and is never visited by the walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_list_section_feeds_list_from_section_without_walking() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## List\n\n\
---\n\n\
- alpha\n\
- beta\n\n\
## Main\n\n\
```lua\n\
local items = list_from_section('## List')\n\
assert(#items == 2 and items[1] == 'alpha' and items[2] == 'beta')\n\
return table.concat(items, ',')\n\
```\n";
    let (result, records) = run_recorded(md).await;
    assert_eq!(
        result.expect("an off-walk list section must feed list_from_section"),
        "alpha,beta"
    );
    assert!(
        records.iter().all(|(_, section, _)| section != "List"),
        "an off-walk list section must never walk: {records:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_exhausted_arm_exposes_failure_metadata() {
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\nmax_tool_iterations: 2\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.need('echo', 'echo tool')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
assert(r[1].ok == false)\n\
assert(r[1].exhausted == true)\n\
assert(r[1].item == 'alpha')\n\
assert(r[1].text:find('tool loop exhausted', 1, true))\n\
assert(tostring(r[1]) == r[1].text)\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\ntools.add('echo')\n```\n\n\
Loop forever on {{ item }}.\n\n\
### Items\n\n\
- alpha\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("soft-degraded fanout must still return structured results");
    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_exposes_section_metadata() {
    let gateway = ScriptedGateway::start(vec![resp_text_finish("alpha-answer", "stop")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Alpha\n\n\
```lua\n\
assert(sys.section_name == 'Alpha')\n\
assert(sys.execution == 'execute-test')\n\
assert(sys.section_count == 2)\n\
local ok = pcall(function() return sys.reply_finish_reason end)\n\
assert(not ok, 'reply_finish_reason must be absent before prose')\n\
```\n\n\
Write one fact.\n\n\
```lua\n\
assert(sys.reply_finish_reason == 'stop')\n\
assert(reply == 'alpha-answer')\n\
```\n\n\
## Beta\n\n\
```lua\n\
assert(sys.section_name == 'Beta')\n\
assert(sys.section_count == 2)\n\
assert(sys.execution == 'execute-test')\n\
return 'done'\n\
```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("sys must expose section metadata");
    assert_eq!(out, "done");
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

#[test]
fn now_rfc3339_checked_produces_a_parseable_timestamp() {
    // F11: timestamp construction is fallible and, on the normal path, yields a
    // valid RFC 3339 string (never silently coerced to empty).
    let now = now_rfc3339_checked().expect("formatting the current time must succeed");
    assert!(!now.is_empty(), "a formatted timestamp is never empty");
    // RFC 3339 shape: `YYYY-MM-DDThh:mm:ss...` with a `T` date/time separator and
    // a UTC designator (the formatter renders UTC, so `Z` or a `+00:00` offset).
    assert!(now.contains('T'), "RFC 3339 has a T separator: {now}");
    assert!(
        now.ends_with('Z') || now.contains('+'),
        "RFC 3339 UTC has a zone designator: {now}"
    );
}
