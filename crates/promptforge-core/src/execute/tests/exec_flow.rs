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

    // The subroutine sits after its run-ending caller: a contained chain
    // falls through like any walk, so a subroutine placed before later
    // sections would pull them into the chain.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
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
```\n\n\
## Research\n\n\
```lua\n\
local step = tasks['## Research']\n\
assert(step.name == 'Research')\n\
assert(step.has_prose == true)\n\
```\n\n\
Research {{ args }}.\n\n\
```lua\nstore.write('evidence.md', reply)\n```\n";
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

/// A jump carries the prior section's reply across the transfer: the target
/// sees the model reply the jumper's prose produced.
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

/// A jump inside `execute()` is contained by the chain: followed, not
/// rejected (the retired reject policy's inversion). The chain's index moves
/// to the target - the sections between the jumper and the target do not
/// run - and the target's reply returns to the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_execute_is_contained_in_the_chain() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local r = execute('## Sub')\n\
return 'main:' .. r\n\
```\n\n\
## Sub\n\n\
```lua\n\
jump('## Peer')\n\
```\n\n\
## Skipped\n\n\
```lua\n\
error('the chain jump must move past me')\n\
```\n\n\
## Peer\n\n\
```lua\n\
return 'peer-ran'\n\
```\n";
    let out = run_offline(md)
        .await
        .expect("a jump inside execute must be followed within the chain");
    assert_eq!(out, "main:peer-ran");
}

/// The canonical contained chain (decision 14): A executes Sub; Sub jumps to
/// its child S1, starting a child-level chain that falls through to S2; when
/// S2 finishes, the chain's final reply returns to A and the outer walk
/// continues at B, never having moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_chain_jumps_to_a_child_and_returns_the_chain_reply() {
    let gateway = ScriptedGateway::start(vec![resp_text("reply-s1"), resp_text("reply-s2")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
store.append('order.txt', 'A1\\n')\n\
local r = execute('## Sub')\n\
assert(r == 'reply-s2', 'the chain final reply returns to A')\n\
store.append('order.txt', 'A2\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n\n\
## Sub\n\n\
```lua\n\
store.append('order.txt', 'Sub\\n')\n\
jump('### S1')\n\
```\n\n\
### S1\n\n\
Ask S1.\n\n\
```lua\n\
assert(reply == 'reply-s1', 'the chain rolls the reply forward')\n\
store.append('order.txt', 'S1\\n')\n\
```\n\n\
### S2\n\n\
```lua\n\
assert(reply == 'reply-s1', 'fall-through inside the chain carries the reply')\n\
```\n\n\
Ask S2.\n\n\
```lua\n\
store.append('order.txt', 'S2\\n')\n\
```\n";
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
    .expect("the execute chain must jump, fall through, and return its reply");
    assert_eq!(out, "A1\nSub\nS1\nS2\nA2\nB\n");
}

/// The second canonical example (decision 14): A executes the off-walk S1,
/// which runs because it is addressed; the chain falls through to S2, and
/// S2's reply returns to A. The main walk ends at B and never runs S1 or S2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_chain_over_off_walk_siblings_returns_to_the_caller() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local r = execute('## S1')\n\
store.append('order.txt', 'A:' .. r .. '\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n\n\
## S1\n\n\
---\n\n\
```lua\n\
store.append('order.txt', 'S1\\n')\n\
```\n\n\
## S2\n\n\
```lua\n\
store.append('order.txt', 'S2\\n')\n\
return 's2-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the chain must run the addressed off-walk target and fall through");
    assert_eq!(out, "S1\nS2\nA:s2-reply\nB\n");
}

/// A jump inside an `execute()` chain to a sibling moves within the
/// contained chain: the walk continues from the jump target under the normal
/// rules, and the chain's final reply is the call's return value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_an_execute_chain_moves_within_the_chain() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local r = execute('## Sub')\n\
return 'A:' .. r\n\
```\n\n\
## Sub\n\n\
```lua\n\
jump('## Peer')\n\
```\n\n\
## Peer\n\n\
```lua\n\
store.append('order.txt', 'Peer\\n')\n\
```\n\n\
## Tail\n\n\
```lua\n\
store.append('order.txt', 'Tail\\n')\n\
return 'tail-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a jump inside the chain must move within the chain");
    assert_eq!(out, "A:tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Peer\nTail\n");
}

/// The outer walk never moves while a contained chain runs: wherever the
/// chain ends, the outer walk resumes at the section after the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_outer_walk_never_moves_during_a_contained_chain() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
execute('## Sub')\n\
store.append('order.txt', 'A-done\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n\n\
## Sub\n\n\
```lua\n\
jump('## Peer')\n\
```\n\n\
## Peer\n\n\
```lua\n\
store.append('order.txt', 'Peer\\n')\n\
return 'p'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the outer walk must resume at the section after the caller");
    assert_eq!(out, "Peer\nA-done\nB\n");
}

/// A contained chain skips off-walk sections in fall-through like any walk:
/// only addressing runs a marked section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contained_chain_skips_off_walk_sections_in_fall_through() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local r = execute('## Sub')\n\
return r\n\
```\n\n\
## Sub\n\n\
```lua\n\
store.append('order.txt', 'Sub\\n')\n\
```\n\n\
## Hidden\n\n\
---\n\n\
```lua\n\
store.append('order.txt', 'Hidden\\n')\n\
```\n\n\
## Tail\n\n\
```lua\n\
store.append('order.txt', 'Tail\\n')\n\
return 'tail-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the chain must skip the off-walk section in fall-through");
    assert_eq!(out, "tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nTail\n");
}

/// A return inside a contained chain ends the chain, not the run: the
/// returned value is the call's return, the chain's remaining sections do
/// not run, and the outer walk continues.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local r = execute('## Sub')\n\
store.append('order.txt', 'A:' .. r .. '\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n\n\
## Sub\n\n\
```lua\n\
return 'sub-reply'\n\
```\n\n\
## After\n\n\
```lua\n\
error('a return must end the chain before fall-through')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a return must end the chain, not the run");
    assert_eq!(out, "A:sub-reply\nB\n");
}

/// A contained chain counts its own `sys.id` from 1, like a fresh run, and
/// its entries do not advance the outer chain's count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contained_chain_counts_sys_id_from_one() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
assert(sys.id == 1, 'the outer chain counts its own sections')\n\
local r = execute('## Sub')\n\
store.append('order.txt', r .. '\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
assert(sys.id == 2, 'the contained chain must not advance the outer count')\n\
return store.read('order.txt')\n\
```\n\n\
## Sub\n\n\
```lua\n\
assert(sys.id == 1, 'a contained chain counts sys.id from 1')\n\
```\n\n\
## Tail\n\n\
```lua\n\
assert(sys.id == 2, 'the chain counts its own fall-through')\n\
return 'tail-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a contained chain must count its own sys.id from 1");
    assert_eq!(out, "tail-reply\n");
}

/// Nested `execute()` is capped at [`MAX_EXECUTE_DEPTH`]. Locks the
/// `execute_depth` divergence threaded through the unified engine (the
/// top-level walk always enters at depth 0; the subroutine carries its depth).
/// The caller is not in its own visible set, so the recursion is mutual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_recursion_is_capped() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\nreturn execute('## B')\n```\n\n\
## B\n\n\
```lua\nreturn execute('## A')\n```\n";
    let error = run_offline(md)
        .await
        .expect_err("unbounded execute recursion must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("recursion exceeded cap"),
        "expected a recursion-cap error, got: {rendered}"
    );
}

/// The caller is outside its own visible set (decision 3): naming its own
/// heading resolves as not-found for both `execute` and `jump`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_cannot_address_itself() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Self\n\n\
```lua\nreturn execute('## Self')\n```\n";
    let error = run_offline(md).await.expect_err("self-execute must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );

    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Self\n\n\
```lua\njump('## Self')\n```\n";
    let error = run_offline(md).await.expect_err("self-jump must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_returns_structured_results() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
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

/// A jump to an H3 child starts a child-level walk at the target: it falls
/// through to the target's following siblings under the same rules as the
/// top-level walk, and when the level exhausts the parent walk resumes after
/// the jumper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_a_child_starts_the_child_level_walk() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
store.append('order.txt', 'A\\n')\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
store.append('order.txt', 'X\\n')\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('order.txt', 'Y\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a jump to a child must start the child-level walk");
    assert_eq!(out, "A\nX\nY\nB\n");
}

/// The reply thread follows the detour: the jumper's reply reaches the
/// sub-walk's first section, each section of the sub-walk rolls it forward,
/// and the sub-walk's last reply resumes the parent chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_walk_reply_thread_follows_the_detour() {
    let gateway = ScriptedGateway::start(vec![
        resp_text("reply-a"),
        resp_text("reply-x"),
        resp_text("reply-y"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
Ask A.\n\n\
```lua\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
assert(reply == 'reply-a', 'the jumper reply reaches the first child')\n\
```\n\n\
Ask X.\n\n\
### Y\n\n\
```lua\n\
assert(reply == 'reply-x', 'the child walk rolls the reply forward')\n\
```\n\n\
Ask Y.\n\n\
## B\n\n\
```lua\n\
assert(reply == 'reply-y', 'the sub-walk last reply resumes the parent chain')\n\
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
    .expect("the reply thread must follow the detour");
    assert_eq!(out, "reply-y");
}

/// The child-level rule recurses: a jump from an H3 child to an H4 grandchild
/// starts an H4-level walk, and each level's exhaustion resumes its parent
/// after the jumper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_walk_recurses_to_h4() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
store.append('order.txt', 'A\\n')\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
store.append('order.txt', 'X\\n')\n\
jump('#### P')\n\
```\n\n\
#### P\n\n\
```lua\n\
store.append('order.txt', 'P\\n')\n\
```\n\n\
#### Q\n\n\
```lua\n\
store.append('order.txt', 'Q\\n')\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('order.txt', 'Y\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the child-level rule must recurse to H4");
    assert_eq!(out, "A\nX\nP\nQ\nY\nB\n");
}

/// The off-walk flag applies at every walked level: an off-walk H3 is skipped
/// by the child-level walk's fall-through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_child_is_skipped_by_the_child_walk() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
store.append('order.txt', 'X\\n')\n\
```\n\n\
### Off\n\n\
---\n\n\
```lua\n\
store.append('order.txt', 'Off\\n')\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('order.txt', 'Y\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the child walk must skip the off-walk child");
    assert_eq!(out, "X\nY\n");
}

/// An off-walk child stays addressable: a jump to it runs it (and the
/// fall-through that follows skips nothing addressed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_an_off_walk_child_runs_it() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
jump('### Off')\n\
```\n\n\
### X\n\n\
```lua\n\
store.append('order.txt', 'X\\n')\n\
```\n\n\
### Off\n\n\
---\n\n\
```lua\n\
store.append('order.txt', 'Off\\n')\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('order.txt', 'Y\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a jump to an off-walk child must run it");
    assert_eq!(out, "Off\nY\n");
}

/// `execute` to a child starts a contained chain at the target: the chain
/// falls through to the target's following siblings under the same rules as
/// any walk, and the chain's final reply is the call's return value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_to_a_child_starts_a_contained_chain() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local r = execute('### Sub')\n\
return 'got:' .. r\n\
```\n\n\
### Sub\n\n\
```lua\n\
store.append('order.txt', 'Sub\\n')\n\
```\n\n\
### After\n\n\
```lua\n\
store.append('order.txt', 'After\\n')\n\
return 'after-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("execute to a child must start a contained chain");
    assert_eq!(out, "got:after-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

/// The top-level walk never descends: a section's children do not run unless
/// addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_never_descends_into_children() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
store.append('order.txt', 'A\\n')\n\
```\n\n\
### Child\n\n\
```lua\n\
error('a child must not run by fall-through')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('order.txt', 'B\\n')\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the walk must never descend into children");
    assert_eq!(out, "A\nB\n");
}

/// A running child's visible set is its own siblings plus its own children:
/// it can execute a child and jump to a sibling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_child_addresses_its_own_siblings_and_children() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
local r = execute('#### Grand')\n\
store.append('order.txt', 'X:' .. r .. '\\n')\n\
jump('### Y')\n\
```\n\n\
#### Grand\n\n\
```lua\n\
return 'grand-ran'\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('order.txt', 'Y\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
return store.read('order.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a running child must address its own siblings and children");
    assert_eq!(out, "X:grand-ran\nY\n");
}

/// A running child cannot address a top-level section: the parent level is
/// not in its visible set, so the jump resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_child_cannot_address_a_top_level_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
jump('## B')\n\
```\n\n\
## B\n\n\
```lua\n\
return 'b-ran'\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("a child jumping to a top-level section must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a top-level section is not in a child's visible set: {rendered}"
    );
}

/// A sibling's child (a niece or nephew) is not in the visible set: the jump
/// resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_a_niece_errors() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
jump('### Niece')\n\
```\n\n\
## B\n\n\
### Niece\n\n\
```lua\n\
return 'niece-ran'\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("a jump to a niece must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

/// `sys.id` counts the sections the walk has entered run-wide: the detour
/// into a child level continues the count rather than restarting it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_id_counts_sections_entered_run_wide() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
```\n\n\
### Y\n\n\
```lua\n\
store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
return store.read('ids.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("sys.id must count sections entered run-wide");
    assert_eq!(out, "1\n2\n3\n4\n");
}

/// An off-walk child section still runs as a fanout worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_worker_runs_as_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
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
        .split_once("available sections:")
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

/// Two top-level sections sharing one name error loudly as ambiguous rather
/// than silently resolving to the first (the retired `resolve_h2_section`
/// first-match behavior). Unreachable through a real prompt (the parser
/// forbids duplicate sibling names), so the walk's jump-target resolution is
/// exercised directly with a synthetic sibling slice.
#[test]
fn duplicate_top_level_section_names_error_loudly() {
    fn top(name: &str) -> crate::parser::Section {
        crate::parser::Section {
            name: name.to_string(),
            level: 2,
            blocks: Vec::new(),
            children: Vec::new(),
            items: Vec::new(),
            off_walk: false,
        }
    }
    let sections = vec![top("Main"), top("Dup"), top("Dup")];
    let error = super::super::engine::resolve_jump_target("## Dup", &sections, &sections[0])
        .expect_err("two visible sections with one name must be ambiguous");
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
local r = fanout('### Worker', list_from_section('### Items'))\n\
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

/// Inside a fanout arm the global is a loud stub, same as list/fanout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_is_not_available_inside_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
execute('### Items')\n\
return item\n\
```\n\n\
### Items\n\n\
- alpha\n";
    let error = run_offline(md)
        .await
        .expect_err("execute inside an arm must fail loudly");
    let rendered = error.to_string();
    assert!(
        rendered.contains("execute() is not available inside a fanout arm"),
        "error was: {rendered}"
    );
}

/// Inside a fanout arm the global is a loud stub, same as list/execute.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_is_not_available_inside_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
fanout('### Worker', {})\n\
return item\n\
```\n\n\
### Items\n\n\
- alpha\n";
    let error = run_offline(md)
        .await
        .expect_err("fanout inside an arm must fail loudly");
    let rendered = error.to_string();
    assert!(
        rendered.contains("fanout() is not available inside a fanout arm"),
        "error was: {rendered}"
    );
}

/// A jump records into the arm VM's slot and is rejected at the boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_is_rejected_inside_a_fanout_arm() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
jump('### Items')\n\
```\n\n\
### Items\n\n\
- alpha\n";
    let error = run_offline(md)
        .await
        .expect_err("jump inside an arm must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("jump(### Items) is not allowed inside a fanout arm"),
        "error was: {rendered}"
    );
}

/// A walked section never sees the fanout `item` global: the seed split
/// installs `item` only for an arm, so a regression seeding it on the walk
/// path must fail here.
#[tokio::test]
async fn item_global_is_absent_in_a_walked_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
assert(item == nil, 'a walked section must not see the fanout item global')\n\
return 'ok'\n\
```\n";
    let out = run_offline(md)
        .await
        .expect("a walked section runs with no item global");
    assert_eq!(out, "ok");
}

/// The arm's `var` starts as a fresh table: the top-level H1 hand-off seeds
/// the walk's sections, never an arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_var_is_fresh_not_the_h1_handoff() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
```lua\n\
var.from_h1 = 'seeded'\n\
```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
assert(var.from_h1 == nil, 'an arm var must start fresh')\n\
return item\n\
```\n\n\
### Items\n\n\
- alpha\n";
    let out = run_offline(md)
        .await
        .expect("the arm runs with a fresh var");
    assert_eq!(out, "alpha");
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
local r = fanout('### Worker', list_from_section('### Items'))\n\
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

/// Array members arrive as themselves, in collection order: a string stays a
/// string, a number a number, a boolean a boolean, a nested table a table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_array_members_arrive_as_themselves_in_order() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'b', 2, true, {nested='x'}})\n\
assert(#r == 4)\n\
assert(r[1].text == 'string:b')\n\
assert(r[2].text == 'number:2')\n\
assert(r[3].text == 'boolean:true')\n\
assert(r[4].text == 'table:x')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\n\
if type(item) == 'table' then return 'table:' .. item.nested end\n\
return type(item) .. ':' .. tostring(item)\n\
```\n";
    let out = run_offline(md)
        .await
        .expect("array members must arrive as themselves");
    assert_eq!(out, "ok");
}

/// Hash members arrive as pair tables (`item.key` / `item.value`), and
/// `.item` on each arm result carries the same pair table back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_hash_members_arrive_as_pair_tables() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {alpha=1, beta='two'})\n\
assert(#r == 2)\n\
local seen = {}\n\
for i = 1, #r do\n\
  assert(type(r[i].item) == 'table')\n\
  assert(r[i].item.key ~= nil and r[i].item.value ~= nil)\n\
  seen[r[i].text] = true\n\
end\n\
assert(seen['alpha=1'] and seen['beta=two'])\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item.key .. '=' .. tostring(item.value)\n```\n";
    let out = run_offline(md)
        .await
        .expect("hash members must arrive as pair tables");
    assert_eq!(out, "ok");
}

/// An empty collection maps to an empty result table: mapping over zero
/// members is legitimate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_empty_returns_an_empty_table() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {})\n\
assert(#r == 0)\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n";
    let out = run_offline(md)
        .await
        .expect("an empty collection must return an empty table");
    assert_eq!(out, "ok");
}

/// `.item` on each arm result carries the member value back as a Lua value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_item_round_trips_members() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {1, 'two', {n=3}})\n\
assert(r[1].item == 1)\n\
assert(r[2].item == 'two')\n\
assert(type(r[3].item) == 'table' and r[3].item.n == 3)\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn 'done'\n```\n";
    let out = run_offline(md)
        .await
        .expect(".item must round-trip each member");
    assert_eq!(out, "ok");
}

/// A worker resolves as a sibling: a top-level worker marked off-walk is
/// shared infrastructure the walk never visits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_resolves_as_a_sibling() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local r = fanout('## Worker', {'x'})\n\
return r[1].text\n\
```\n\n\
## Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n";
    let out = run_offline(md)
        .await
        .expect("a sibling worker must resolve");
    assert_eq!(out, "x-done");
}

/// One off-walk sibling worker is shared by two sibling callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_shared_by_two_sibling_callers() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local r = fanout('## Worker', {'a'})\n\
store.write('a.txt', r[1].text)\n\
```\n\n\
## B\n\n\
```lua\n\
local r = fanout('## Worker', {'b'})\n\
return store.read('a.txt') .. ',' .. r[1].text\n\
```\n\n\
## Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n";
    let out = run_offline(md)
        .await
        .expect("one worker must serve two sibling callers");
    assert_eq!(out, "a-done,b-done");
}

/// A sibling's child (a niece) is outside the caller's visible set and
/// resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_niece_worker_is_not_visible() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
fanout('### Niece', {'x'})\n\
```\n\n\
## Other\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
```lua\nreturn item\n```\n";
    let error = run_offline(md)
        .await
        .expect_err("a niece worker must not be visible");
    assert!(
        error.to_string().contains("not found"),
        "error was: {error}"
    );
}

/// The retired two-string form errors at the boundary, pointing at
/// `list_from_section`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_string_second_parameter_errors_pointing_at_list_from_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
fanout('### Worker', '### Items')\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n\n\
### Items\n\n\
- alpha\n";
    let error = run_offline(md)
        .await
        .expect_err("the two-string form must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("fanout's second parameter is a collection"),
        "error was: {rendered}"
    );
    assert!(
        rendered.contains("list_from_section"),
        "the error must point at list_from_section: {rendered}"
    );
}

/// A number or boolean second parameter is not a collection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_number_and_boolean_second_parameters_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local ok_n, err_n = pcall(fanout, '### Worker', 5)\n\
assert(not ok_n and tostring(err_n):find('collection'), tostring(err_n))\n\
local ok_b, err_b = pcall(fanout, '### Worker', true)\n\
assert(not ok_b and tostring(err_b):find('collection'), tostring(err_b))\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n";
    let out = run_offline(md)
        .await
        .expect("number and boolean second parameters must error");
    assert_eq!(out, "ok");
}

/// A function member cannot cross into an arm; the error names its index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_function_member_errors_naming_the_index() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
fanout('### Worker', {'a', function() end})\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n";
    let error = run_offline(md)
        .await
        .expect_err("a function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 2"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");
}

/// A non-scalar (table) key cannot be represented; the error says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_table_keyed_member_errors() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local t = {}\n\
t[{}] = 'x'\n\
fanout('### Worker', t)\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n";
    let error = run_offline(md).await.expect_err("a table key must error");
    assert!(
        error
            .to_string()
            .contains("key must be a string, number, or boolean"),
        "error was: {error}"
    );
}

/// The item cap bounds the collection length: 1025 members exceed the
/// default 1024 cap before any arm is scheduled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_oversized_collection_errors() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local t = {}\n\
for i = 1, 1025 do t[i] = i end\n\
fanout('### Worker', t)\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n";
    let error = run_offline(md)
        .await
        .expect_err("an oversized collection must error");
    assert!(
        error.to_string().contains("exceeding the maximum of 1024"),
        "error was: {error}"
    );
}

/// A list section is not a worker template; naming one as the worker errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_that_is_a_list_section_errors() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
fanout('### Items', {'x'})\n\
```\n\n\
### Items\n\n\
- a\n\
- b\n";
    let error = run_offline(md)
        .await
        .expect_err("a list section is not a worker template");
    assert!(
        error
            .to_string()
            .contains("is a list section, not a worker template"),
        "error was: {error}"
    );
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
