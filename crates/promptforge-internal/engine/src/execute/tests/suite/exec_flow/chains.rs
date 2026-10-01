//! Contained `call` chains and the jumps inside them, the `sys.id` and
//! `sys.taskid` values chains and fanout arms take, and the recursion and
//! self-address limits.

use super::*;

/// A jump transfers control and the jumper's remaining blocks never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_transfer_skips_the_jumpers_remaining_blocks() {
    let md = flow_prompt!(
        "\
## Check\n\n\
```lua\n\
store.write('seen.txt', 'check')\n\
jump('## Help')\n\
store.write('seen.txt', 'should-not-run')\n\
```\n\n\
## Accept\n\n\
```lua\nreturn 'accepted'\n```\n\n\
## Help\n\n\
```lua\n\
return 'helped:' .. store.read('seen.txt')\n\
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("jump must transfer control");
    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

/// A jump inside `call()` is contained by the chain: followed, not
/// rejected. The chain's index moves to the target - the sections between
/// the jumper and the target do not run - and the target's reply returns to
/// the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_call_is_contained_in_the_chain() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local r = call('## Sub')\n\
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
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a jump inside call must be followed within the chain");
    assert_eq!(out, "main:peer-ran");
}

/// The canonical contained chain (decision 14): A calls Sub; Sub jumps to
/// its child S1, starting a child-level chain that falls through to S2; S2's
/// return is the chain's final text back to A, and the outer walk
/// continues at B, never having moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_chain_jumps_to_a_child_and_returns_the_chain_result() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
store.append('order.txt', 'A1\\n')\n\
local r = call('## Sub')\n\
assert(r == 's2-result', 'the chain final text returns to A')\n\
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
```lua\n\
store.append('order.txt', 'S1\\n')\n\
```\n\n\
### S2\n\n\
```lua\n\
store.append('order.txt', 'S2\\n')\n\
return 's2-result'\n\
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the call chain must jump, fall through, and return its final text");
    assert_eq!(out, "A1\nSub\nS1\nS2\nA2\nB\n");
}

/// The second canonical example (decision 14): A executes the off-walk S1,
/// which runs because it is addressed; the chain falls through to S2, and
/// S2's reply returns to A. The main walk ends at B and never runs S1 or S2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_chain_over_off_walk_siblings_returns_to_the_caller() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
local r = call('## S1')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the chain must run the addressed off-walk target and fall through");
    assert_eq!(out, "S1\nS2\nA:s2-reply\nB\n");
}

/// A jump inside a `call()` chain to a sibling moves within the
/// contained chain: the walk continues from the jump target under the normal
/// rules, and the chain's final reply is the call's return value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_inside_a_call_chain_moves_within_the_chain() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
local r = call('## Sub')\n\
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
```\n"
    );
    let store = TestStore::new();
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
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
call('## Sub')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the outer walk must resume at the section after the caller");
    assert_eq!(out, "Peer\nA-done\nB\n");
}

/// A return inside a contained chain ends the chain, not the run: the
/// returned value is the call's return, the chain's remaining sections do
/// not run, and the outer walk continues.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
local r = call('## Sub')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a return must end the chain, not the run");
    assert_eq!(out, "A:sub-reply\nB\n");
}

/// A `call` chain's sections take ids nested under the chain's own id: the
/// contained chain is the walk's child `0.0`, so its entries are `0.0.N`,
/// and the outer walk resumes its own `0.N` sequence when the chain ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_chain_counts_its_own_entries_and_the_outer_walk_resumes_its_own_sequence() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
assert(sys.id == '0.1', 'the first walked section takes entry 1 of the root chain')\n\
local r = call('## Sub')\n\
store.append('order.txt', r .. '\\n')\n\
```\n\n\
## B\n\n\
```lua\n\
assert(sys.id == '0.2', 'the outer walk resumes its own sequence')\n\
return store.read('order.txt')\n\
```\n\n\
## Sub\n\n\
```lua\n\
assert(sys.id == '0.0.0', 'the contained chain is child 0 and starts at entry 0')\n\
```\n\n\
## Tail\n\n\
```lua\n\
assert(sys.id == '0.0.1', 'the chain fall-through takes its next entry')\n\
return 'tail-reply'\n\
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a call chain must take ids nested under its own chain");
    assert_eq!(out, "tail-reply\n");
}

/// Entering the same section twice hands out two distinct `sys.id` values:
/// two call children of the walk are two chains.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entering_the_same_section_twice_takes_two_ids() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local a = call('## Sub')\n\
local b = call('## Sub')\n\
return a .. ',' .. b\n\
```\n\n\
## Sub\n\n\
```lua\n\
return tostring(sys.id)\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("re-entering a section must take a fresh id");
    assert_eq!(out, "0.0.0,0.1.0");
}

/// Fanout arms take unique `sys.id` values nested under the caller's chain
/// (each arm is a child chain, allocated in collection order at dispatch,
/// so the ids never depend on finish order) and a per-fanout 1-based
/// `sys.index`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arms_take_child_ids_in_collection_order_and_a_per_fanout_index() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'a', 'b'})\n\
local function parts(s) return string.match(s, '^(%d+):([%d%.]+)$') end\n\
local i1, id1 = parts(r[1].text)\n\
local i2, id2 = parts(r[2].text)\n\
assert(i1 == '1' and i2 == '2', 'sys.index is the 1-based per-fanout position')\n\
assert(id1 ~= id2, 'arms take unique ids')\n\
assert(id1 == '0.0.0' and id2 == '0.1.0', 'arm ids are the caller children in collection order')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\n\
return tostring(sys.index) .. ':' .. tostring(sys.id)\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("arms must take child ids in collection order and a per-fanout index");
    assert_eq!(out, "ok");
}

/// `sys.index` exists only inside a fanout arm; reading it in a walked
/// section raises the sealed-sys unknown-field error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_index_outside_a_fanout_errors() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
return tostring(sys.index)\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("sys.index outside a fanout must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown sys field 'index'"),
        "sys.index is fanout-only: {rendered}"
    );
}

/// `sys.taskid` is the nearest enclosing task: a fanout arm is a task the
/// `fanout` shim spawns, so it reports its own id - the caller's first
/// child - while the main walk stays task `0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_taskid_inside_a_fanout_arm_is_the_arms_own_task() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
assert(sys.taskid == '0', 'the main walk is task 0, got ' .. sys.taskid)\n\
local results = fanout('### Worker', {'a'})\n\
return results[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
return sys.taskid\n\
```\n"
    );
    let out = run_offline(md).await.expect("an arm reads its own task id");
    assert_eq!(out, "0.0");
}

/// Nested `call()` is capped at [`MAX_CALL_DEPTH`]. Locks the
/// `call_depth` divergence threaded through the one shared walk (the
/// top-level walk always enters at depth 0; the subroutine keeps its depth).
/// The caller is not in its own visible set, so the recursion is mutual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_recursion_is_capped() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\nreturn call('## B')\n```\n\n\
## B\n\n\
```lua\nreturn call('## A')\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("unbounded call recursion must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("recursion exceeded cap"),
        "expected a recursion-cap error, got: {rendered}"
    );
}

/// The caller is outside its own visible set (decision 3): naming its own
/// heading to `call` resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_cannot_call_itself() {
    let md = flow_prompt!(
        "\
## Self\n\n\
```lua\nreturn call('## Self')\n```\n"
    );
    let error = run_offline(md).await.expect_err("self-call must fail");
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
    let md = flow_prompt!(
        "\
## Self\n\n\
```lua\njump('## Self')\n```\n"
    );
    let error = run_offline(md).await.expect_err("self-jump must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}
