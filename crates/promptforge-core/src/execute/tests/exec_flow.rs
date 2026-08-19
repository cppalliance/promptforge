use super::super::*;
use super::run;
use super::*;

/// Alternating lua/prose blocks run in order; non-final prose is single-shot,
/// final prose loops, and trailing lua sees the last reply.
#[tokio::test]
async fn section_with_alternating_blocks_executes_in_order() {
    let gateway = ScriptedGateway::start(vec![resp_text("reply-1"), resp_text("reply-2")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\nstore.append('order.txt', 'lua1\\n')\n```\n\n\
First ask.\n\n\
```lua\nstore.append('order.txt', 'lua2\\n')\n```\n\n\
Final ask.\n\n\
```lua\nstore.append('order.txt', 'lua3\\n')\nreturn reply\n```\n";
    let store = StoreRef::memory();
    let out = run(&bound_for_model(md), "", &[], &store, gatewayed(addr))
        .await
        .expect("alternating blocks must execute");

    assert_eq!(out, "reply-2");
    assert_eq!(gateway.call_count(), 2);
    assert_eq!(
        store.read("order.txt").expect("order log"),
        "lua1\nlua2\nlua3\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_runs_named_section_as_subroutine() {
    let gateway = ScriptedGateway::start(vec![resp_text("research-reply")]).await;
    let addr = gateway.addr();

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
    let out = run(&bound_for_model(md), "topic", &[], &store, gatewayed(addr))
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
        gatewayed(addr),
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
    let out = run(&bound_for_model(md), "", &[], &store, gatewayed(addr))
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
/// heading to `execute` resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_cannot_execute_itself() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Self\n\n\
```lua\nreturn execute('## Self')\n```\n";
    let error = run_offline(md).await.expect_err("self-execute must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

/// The caller is outside its own visible set (decision 3): naming its own
/// heading to `jump` resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_cannot_jump_to_itself() {
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
        gatewayed(addr),
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

/// A synthetic section for the resolution-helper tests, which exercise
/// duplicate-name sets that are unreachable through a real prompt (the parser
/// forbids duplicate sibling names).
fn synthetic_section(name: &str, level: u8, items: Vec<String>) -> crate::parser::Section {
    crate::parser::Section {
        name: name.to_string(),
        level,
        blocks: Vec::new(),
        children: Vec::new(),
        items,
        off_walk: false,
    }
}

/// Two visible sections sharing one `(level, name)` address error loudly as
/// ambiguous rather than silently resolving to the first.
#[test]
fn list_from_section_ambiguous_error_is_loud() {
    let visible = vec![
        synthetic_section("Dup", 3, vec!["x".to_string()]),
        synthetic_section("Dup", 3, vec!["x".to_string()]),
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
        synthetic_section("Main", 2, Vec::new()),
        synthetic_section("Dup", 2, Vec::new()),
        synthetic_section("Dup", 2, Vec::new()),
    ];
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

/// The scaffold the fanout-arm capability tests share: a `## Parent` that
/// fans `### Worker` out over one `alpha` member and returns the first arm's
/// text. The worker body and its sibling sections follow.
const ARM_FANOUT_PARENT: &str = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha'})\n\
return r[1].text\n\
```\n\n";

/// `list_from_section` inside a fanout arm reads a list section's items,
/// resolving over the worker's visible set (the set the worker was resolved
/// from, minus the worker, plus its children).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_inside_a_fanout_arm_reads_items() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
local items = list_from_section('### Items')\n\
return item .. ':' .. table.concat(items, ',')\n\
```\n\n\
### Items\n\n\
- x\n\
- y\n",
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("list_from_section inside an arm must read items");
    assert_eq!(out, "alpha:x,y");
}

/// `execute` inside a fanout arm runs a contained chain over the worker's
/// visible set: the chain counts its own `sys.id` from 1, runs as plain
/// sections (no `item` seed), and its final reply is the call's return value.
/// The arm and the contained chain also see the run's `tasks` table and
/// `sys.section_count`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_inside_a_fanout_arm_runs_a_contained_chain() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
assert(tasks['## Parent'].name == 'Parent', 'the arm sees the run tasks table')\n\
assert(sys.section_count == 1, 'the arm sees the run top-level section count')\n\
local got = execute('### Sub')\n\
return 'worker:' .. got .. ':' .. item\n\
```\n\n\
### Sub\n\n\
```lua\n\
assert(sys.id == 1, 'a contained chain counts sys.id from 1')\n\
assert(item == nil, 'a contained chain runs as a plain section')\n\
assert(sys.section_count == 1, 'a contained chain sees the run section count')\n\
```\n\n\
### Tail\n\n\
```lua\n\
return 'tail-reply'\n\
```\n",
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("execute inside an arm must run a contained chain");
    assert_eq!(out, "worker:tail-reply:alpha");
}

/// `fanout` inside a fanout arm maps over a collection: the nested worker
/// resolves over the outer worker's visible set, and the nested structured
/// results come back to the outer arm. A nested fanout over an empty
/// collection returns an empty table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_inside_a_fanout_arm_maps_over_a_collection() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Outer', {'a', 'b'})\n\
return table.concat({r[1].text, r[2].text}, ';')\n\
```\n\n\
### Outer\n\n\
```lua\n\
local inner = fanout('### Inner', {item .. '1', item .. '2'})\n\
assert(inner[1].ok and inner[2].ok)\n\
local empty = fanout('### Inner', {})\n\
assert(#empty == 0, 'a nested fanout over an empty collection returns an empty table')\n\
return item .. ':' .. inner[1].text .. ',' .. inner[2].text\n\
```\n\n\
### Inner\n\n\
```lua\n\
assert(sys.taskid ~= nil, 'a nested arm keeps its own taskid')\n\
return item .. '!'\n\
```\n";
    let out = run_offline(md)
        .await
        .expect("fanout inside an arm must map over the collection");
    assert_eq!(out, "a:a1!,a2!;b:b1!,b2!");
}

/// A jump inside a fanout arm transfers control: the arm's remaining blocks
/// are skipped, a child walk runs from the target under the engine's
/// chain-slice rule (counting its own `sys.id` from 1, falling through to the
/// target's following siblings), and the child walk's reply becomes the arm's
/// text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_a_fanout_arm_drives_a_child_walk() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
jump('### Target')\n\
error('the arm remaining blocks are skipped')\n\
```\n\n\
### Target\n\n\
```lua\n\
assert(sys.id == 1, 'the child walk counts its own sys.id from 1')\n\
store.append('order.txt', 'Target\\n')\n\
```\n\n\
### Tail\n\n\
```lua\n\
store.append('order.txt', 'Tail\\n')\n\
return 'tail-reply'\n\
```\n",
    ]
    .concat();
    let store = StoreRef::memory();
    let out = run(&fixture(&md), "", &[], &store, silent())
        .await
        .expect("a jump inside an arm must drive a child walk");
    assert_eq!(out, "tail-reply");
    assert_eq!(
        store.read("order.txt").expect("order log"),
        "Target\nTail\n",
        "the child walk runs the target and falls through to its siblings"
    );
}

/// A jump from an arm into one of the worker's own children drives the
/// child-level walk over the worker's child slice: the target runs with the
/// child walk's own `sys.id` count and no `item` seed, and the walk falls
/// through to the target's child siblings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_a_fanout_arm_to_a_worker_child_walks_the_child_slice() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
jump('#### Child')\n\
error('the arm remaining blocks are skipped')\n\
```\n\n\
#### Child\n\n\
```lua\n\
assert(sys.id == 1, 'the child walk counts its own sys.id from 1')\n\
assert(item == nil, 'the child walk runs as a plain section')\n\
store.append('order.txt', 'Child\\n')\n\
```\n\n\
#### ChildTail\n\n\
```lua\n\
store.append('order.txt', 'ChildTail\\n')\n\
return 'child-tail-reply'\n\
```\n",
    ]
    .concat();
    let store = StoreRef::memory();
    let out = run(&fixture(&md), "", &[], &store, silent())
        .await
        .expect("a jump to a worker child must drive the child slice");
    assert_eq!(out, "child-tail-reply");
    assert_eq!(
        store.read("order.txt").expect("order log"),
        "Child\nChildTail\n",
        "the child walk runs the target and falls through to its child siblings"
    );
}

/// A jump-started child walk that produces no return and no reply exhausts:
/// the arm's text is the empty string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_a_fanout_arm_to_a_silent_chain_returns_empty_text() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
jump('### Target')\n\
```\n\n\
### Target\n\n\
```lua\n\
store.append('order.txt', 'Target\\n')\n\
```\n",
    ]
    .concat();
    let store = StoreRef::memory();
    let out = run(&fixture(&md), "", &[], &store, silent())
        .await
        .expect("a jump to a silent chain must succeed with empty text");
    assert_eq!(
        out, "",
        "an exhausted child walk with no reply maps to empty text"
    );
    assert_eq!(store.read("order.txt").expect("order log"), "Target\n");
}

/// A jump from an arm to a heading outside the worker's visible set (the
/// top-level parent) resolves as not-found, and the error lists only the
/// worker's visible set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_a_fanout_arm_to_a_non_visible_heading_errors() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
jump('## Parent')\n\
```\n\n\
### Items\n\n\
- x\n",
    ]
    .concat();
    let error = run_offline(&md)
        .await
        .expect_err("a jump to a non-visible heading must fail the arm");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("not found"),
        "a non-visible heading must not resolve: {rendered}"
    );
    assert!(
        rendered.contains("### Items"),
        "the error lists the worker's visible set: {rendered}"
    );
}

/// `execute` and `list_from_section` inside an arm naming a section outside
/// the worker's visible set both error not-found; the arm catches them with
/// `pcall` and asserts on the messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_and_list_inside_a_fanout_arm_reject_non_visible_sections() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
local ok_exec, err_exec = pcall(execute, '## Parent')\n\
assert(not ok_exec, 'execute of a non-visible section must fail')\n\
assert(tostring(err_exec):find('not found', 1, true), 'execute error must be not-found: ' .. tostring(err_exec))\n\
local ok_list, err_list = pcall(list_from_section, '## Parent')\n\
assert(not ok_list, 'list_from_section of a non-visible section must fail')\n\
assert(tostring(err_list):find('not found', 1, true), 'list error must be not-found: ' .. tostring(err_list))\n\
return item .. ':rejected'\n\
```\n\n\
### Items\n\n\
- x\n",
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("non-visible execute/list inside an arm must error not-found");
    assert_eq!(out, "alpha:rejected");
}

/// Recursion depth accumulates across a fanout boundary: a section at the
/// execute cap cannot fan out, because its arms would run one level deeper
/// still. Mutual `execute` recursion drives the depth to the cap (a worker's
/// visible set never contains the worker, so only an execute chain can reach
/// the cap); the store counter switches the last recursion step to `fanout`,
/// which must trip the same cap at the boundary rather than resetting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_recursion_across_the_boundary_trips_the_depth_cap() {
    // A runs at depths 0, 2, 4, 6, 8 (its 5th run); the fanout there would
    // spawn arms at depth 9, past MAX_EXECUTE_DEPTH (8).
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
local ok, v = pcall(store.read, 'n.txt')\n\
local n = tonumber(ok and v or '0') + 1\n\
store.write('n.txt', tostring(n))\n\
if n >= 5 then\n\
  return fanout('## W', {'x'})\n\
end\n\
return execute('## B')\n\
```\n\n\
## B\n\n\
```lua\n\
return execute('## A')\n\
```\n\n\
## W\n\n\
---\n\n\
```lua\n\
return item\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("a fanout at the execute cap must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("fanout recursion exceeded cap of 8"),
        "expected the fanout recursion-cap error, got: {rendered}"
    );
}

/// An arm runs one execute level deeper than its fanout caller: an arm
/// spawned at the cap's edge trips MAX_EXECUTE_DEPTH on its OWN `execute`.
/// Dropping the `+ 1` from the arm's depth in `run_fanout_arms` must fail
/// this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_inside_an_arm_spawned_near_the_cap_trips_the_depth_cap() {
    // B runs at depths 1, 3, 5, 7 (its 4th run); the fanout there spawns arms
    // at depth 8, and the arm's own `execute` would need depth 9 - past
    // MAX_EXECUTE_DEPTH (8).
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## A\n\n\
```lua\n\
return execute('## B')\n\
```\n\n\
## B\n\n\
```lua\n\
local ok, v = pcall(store.read, 'n.txt')\n\
local n = tonumber(ok and v or '0') + 1\n\
store.write('n.txt', tostring(n))\n\
if n >= 4 then\n\
  local r = fanout('## W', {'x'})\n\
  return r[1].text\n\
end\n\
return execute('## A')\n\
```\n\n\
## W\n\n\
---\n\n\
```lua\n\
return execute('## C')\n\
```\n\n\
## C\n\n\
---\n\n\
```lua\n\
return 'c-reply'\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("an execute inside an arm spawned at the cap's edge must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("execute recursion exceeded cap of 8"),
        "expected the execute recursion-cap error, got: {rendered}"
    );
}

/// A multi-prose worker runs the shared block walk: every prose block reaches
/// the model, the conversation rolls forward across blocks, and `{{ reply }}`
/// substitutes the previous block's model text. `{{ item }}` resolves against
/// the arm's collection member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_runs_every_prose_block_with_the_reply_rolling_forward() {
    let gateway =
        ScriptedGateway::start(vec![resp_text("first answer"), resp_text("second answer")]).await;
    let addr = gateway.addr();
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
First ask about {{ item }}.\n\n\
```lua\nvar.mid = 1\n```\n\n\
The first answer was: {{ reply }}.\n",
    ]
    .concat();
    let out = run(
        &bound_for_model(&md),
        "",
        &[],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .expect("a multi-prose worker must run every prose block");
    assert_eq!(out, "second answer", "the arm's text is the final reply");

    let bodies = gateway.requests();
    assert_eq!(
        bodies.len(),
        2,
        "one model turn per prose block: {bodies:?}"
    );
    let first = bodies[0]["messages"].as_array().expect("messages array");
    let first_content = first.last().expect("a user turn")["content"]
        .as_str()
        .expect("content string");
    assert!(
        first_content.contains("First ask about alpha."),
        "the first prose must substitute the arm's item: {first_content}"
    );
    // The conversation rolls forward: the second block's request still carries
    // the first block's user turn, and `{{ reply }}` substituted the first
    // block's model text (the engine binds text replies to `reply` rather
    // than appending them as assistant turns).
    let second = bodies[1]["messages"].as_array().expect("messages array");
    let user_turns: Vec<&str> = second
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert_eq!(
        user_turns,
        vec![
            "First ask about alpha.",
            "The first answer was: first answer."
        ],
        "the second turn must carry the rolled-forward conversation and reply: {bodies:?}"
    );
}

/// `tools.add` between a worker's prose blocks rebuilds the effective scope,
/// so the added tool reaches the next model turn inside the arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_tools_add_between_prose_blocks_reaches_the_next_model_turn() {
    let gateway = ScriptedGateway::start(vec![
        resp_text("first"),
        resp_tool_call("c1", "section_tool", "{\"value\":\"x\"}"),
        resp_text("second"),
    ])
    .await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    // `tools.need`/`models.default` are H1-only declarations, so the
    // declarations block is spliced into the shared scaffold ahead of
    // `## Parent`.
    let mut md = ARM_FANOUT_PARENT.replacen(
        "## Parent",
        "# Test prompt\n\n```lua shared\n\
tools.need('section_tool', 'capability')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Parent",
        1,
    );
    md.push_str(
        "### Worker\n\n\
First ask.\n\n\
```lua\ntools.add('section_tool')\n```\n\n\
Second ask.\n",
    );
    let prompt = bound_with_tools(&md, Vec::new());

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .expect("tools.add inside an arm must reach the next model turn");

    assert_eq!(out, "second");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = gateway.requests();
    assert!(
        bodies[0].get("tools").is_none(),
        "the first prose block predates tools.add: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["tools"][0]["function"]["name"], "section_tool",
        "the second prose block must see the added tool: {bodies:?}"
    );
}

/// `model:infer` works inside an arm: the arm installs the infer hook, so a
/// worker's Lua can call the model directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_works_inside_an_arm() {
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.get('writer'):infer('ping about ' .. item)\n\
```\n",
    ]
    .concat();
    let out = run(
        &bound_for_model(&md),
        "",
        &[],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .expect("model:infer inside an arm must run");
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

/// An arm handed no client creates one lazily when its prose needs it: the
/// creation reads the gateway environment, so with the variables unset the run
/// fails with the missing-variable error instead of silently skipping the
/// prose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_without_a_client_creates_one_lazily_when_prose_needs_it() {
    // The missing-variable error only fires on an unconfigured host; with a
    // gateway exported, the lazy creation would succeed and make a real call.
    if !gateway_env_is_unset() {
        return;
    }
    let md = [ARM_FANOUT_PARENT, "### Worker\n\nAsk about {{ item }}.\n"].concat();
    let error = run(
        &bound_for_model(&md),
        "",
        &[],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .expect_err("an arm with no client must attempt lazy creation, not skip its prose");
    let rendered = error.to_string();
    assert!(
        rendered.contains("missing environment variable: PROMPTFORGE_GATEWAY"),
        "the arm must surface the lazy client construction error: {rendered}"
    );
}

/// `model:infer` inside an arm handed no client surfaces the lazy-creation
/// error through the infer hook - a different code path than the prose walk's
/// lazy creation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_without_a_client_surfaces_the_lazy_error() {
    // Same host dependence as the prose variant above: skip on a configured host.
    if !gateway_env_is_unset() {
        return;
    }
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.get('writer'):infer('ping about ' .. item)\n\
```\n",
    ]
    .concat();
    let error = run(
        &bound_for_model(&md),
        "",
        &[],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .expect_err("model:infer in an arm with no client must surface the lazy error");
    let rendered = error.to_string();
    assert!(
        rendered.contains("missing environment variable: PROMPTFORGE_GATEWAY"),
        "the infer hook must surface the lazy client construction error: {rendered}"
    );
}

/// An unknown model alias errors loudly inside an arm, same as on the walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_model_infer_with_an_unknown_alias_errors_loudly() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
return models.get('ghost'):infer('ping')\n\
```\n",
    ]
    .concat();
    let error = run(
        &bound_for_model(&md),
        "",
        &[],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .expect_err("an unknown model alias inside an arm must fail loudly");
    let rendered = error.to_string();
    assert!(
        rendered.contains("models.get alias \"ghost\" was not declared"),
        "the unknown alias must be named: {rendered}"
    );
}

/// A collection larger than the old default item cap (1024) runs through the
/// prompt-level path: the parent's Lua builds the table,
/// `collection_to_items` converts it, and `fanout` dispatches every member.
/// The worker is pure Lua with an immediate return, so the run needs no
/// client and stays fast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_accepts_a_prompt_built_collection_over_the_old_default_cap() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local members = {}\n\
for i = 1, 1025 do members[i] = 'm' .. i end\n\
local r = fanout('### Worker', members)\n\
assert(#r == 1025, 'every member ran')\n\
return r[1025].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
return item\n\
```\n";
    let out = run_offline(md)
        .await
        .expect("a collection over the old default cap must succeed");
    assert_eq!(out, "m1025");
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

/// `{{ item }}` in walked (non-arm) prose is a substitution error: the walk
/// pins `item: None`, so only a fanout arm's prose may reference it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn item_in_walked_prose_is_a_substitution_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
Ask about {{ item }}.\n";
    let error = run_offline(md)
        .await
        .expect_err("{{ item }} outside a fanout arm must fail substitution");
    let rendered = error.to_string();
    assert!(
        rendered.contains("{{ item }} is nil"),
        "the walked section's prose must reject {{ item }}: {rendered}"
    );
}

/// `execute()` on a child heading resolves the target's index within the
/// caller's CHILD slice, not the sibling slice: earlier children do not run,
/// and the chain falls through to the target's following child siblings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_to_a_later_child_runs_the_child_slice_from_that_index() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Main\n\n\
```lua\n\
local r = execute('### Sub2')\n\
return 'got:' .. r\n\
```\n\n\
### Sub1\n\n\
```lua\n\
error('an earlier child must not run')\n\
```\n\n\
### Sub2\n\n\
```lua\n\
store.append('order.txt', 'Sub2\\n')\n\
```\n\n\
### Sub3\n\n\
```lua\n\
store.append('order.txt', 'Sub3\\n')\n\
return 'sub3-reply'\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("execute to a later child must run the child slice from that index");
    assert_eq!(out, "got:sub3-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub2\nSub3\n");
}

/// A worker that neither returns from Lua nor produces prose text falls
/// through with its arm text seeded from the reply incoming to the parent
/// (the engine's pass-through semantic), not an empty string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_without_output_inherits_the_incoming_reply() {
    let gateway = ScriptedGateway::start(vec![resp_text("prior-reply")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n\
Ask for something.\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha'})\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
assert(item == 'alpha')\n\
```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .expect("a no-output arm must inherit the incoming reply");
    assert_eq!(out, "prior-reply");
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
        gatewayed(addr),
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
        gatewayed(addr),
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
