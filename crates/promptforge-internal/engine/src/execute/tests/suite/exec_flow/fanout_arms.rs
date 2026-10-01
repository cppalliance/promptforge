//! Control flow inside a fanout arm: `list_from_section`, `call`, nested
//! `fanout`, and `jump` over the worker's visible set, the depth cap
//! across the arm boundary, and an arm's text when it replies nothing.

use super::*;

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

/// `call` inside a fanout arm runs a contained chain over the worker's
/// visible set: the chain is the arm's child (`0.0.0` under arm `0.0`), runs
/// as plain
/// sections (no `item` seed), and its final reply is the call's return value.
/// The arm and the contained chain also see the run's `sys.section_count`.
/// The run's limit is 1 and the contained chain fans out: while the chain
/// waits on its arms it gives back the slot its arm holds, so its arms can
/// run instead of queueing behind that slot forever. The driver fails a
/// stalled run, and the timeout fails a hung one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_inside_a_fanout_arm_runs_a_contained_chain() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
tasks.concurrency(1)\n\
local r = fanout('### Worker', {'alpha'})\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
assert(sys.section_count == 1, 'the arm sees the run top-level section count')\n\
local got = call('### Sub')\n\
return 'worker:' .. got .. ':' .. item\n\
```\n\n\
### Sub\n\n\
```lua\n\
assert(sys.id == '0.0.0.0', 'a contained chain is the arm chain child and starts at entry 0')\n\
assert(item == nil, 'a contained chain runs as a plain section')\n\
assert(sys.section_count == 1, 'a contained chain sees the run section count')\n\
local leaves = fanout('#### Leaf', {'x', 'y'})\n\
var.leaves = leaves[1].text .. leaves[2].text\n\
```\n\n\
#### Leaf\n\n\
```lua\n\
return item .. '!'\n\
```\n\n\
### Tail\n\n\
```lua\n\
return 'tail-reply:' .. var.leaves\n\
```\n"
    );
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), run_offline(md))
        .await
        .expect("a contained chain's fanout under a limit of 1 must not deadlock")
        .expect("call inside an arm must run a contained chain");
    assert_eq!(out, "worker:tail-reply:x!y!:alpha");
}

/// `fanout` inside a fanout arm maps over a collection: the nested worker
/// resolves over the outer worker's visible set, and the nested structured
/// results come back to the outer arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_inside_a_fanout_arm_maps_over_a_collection() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Outer', {'a', 'b'})\n\
return table.concat({r[1].text, r[2].text}, ';')\n\
```\n\n\
### Outer\n\n\
```lua\n\
local inner = fanout('### Inner', {item .. '1', item .. '2'})\n\
assert(inner[1].ok and inner[2].ok)\n\
return item .. ':' .. inner[1].text .. ',' .. inner[2].text\n\
```\n\n\
### Inner\n\n\
```lua\n\
assert(tostring(sys.index) == string.sub(item, -1), 'a nested fanout restarts sys.index at 1')\n\
return item .. '!'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("fanout inside an arm must map over the collection");
    assert_eq!(out, "a:a1!,a2!;b:b1!,b2!");
}

/// A jump inside a fanout arm transfers control: the arm's remaining blocks
/// are skipped, a child walk runs from the target under the Engine's
/// chain-slice rule (continuing the arm chain's `sys.id` sequence, falling
/// through to the
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
assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')\n\
store.append('order.txt', 'Target\\n')\n\
```\n\n\
### Tail\n\n\
```lua\n\
store.append('order.txt', 'Tail\\n')\n\
return 'tail-reply'\n\
```\n",
    ]
    .concat();
    let store = TestStore::new();
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
/// child-level walk over the worker's child slice: the target takes the arm
/// chain's next `sys.id` and no `item` seed, and the walk falls
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
assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')\n\
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
    let store = TestStore::new();
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
    let store = TestStore::new();
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

/// `call` and `list_from_section` inside an arm naming a section outside
/// the worker's visible set both error not-found; the arm catches them with
/// `pcall` and asserts on the messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_and_list_inside_a_fanout_arm_reject_non_visible_sections() {
    let md = [
        ARM_FANOUT_PARENT,
        "### Worker\n\n\
```lua\n\
local ok_call, err_call = pcall(call, '## Parent')\n\
assert(not ok_call, 'call of a non-visible section must fail')\n\
assert(tostring(err_call):find('not found', 1, true), 'call error must be not-found: ' .. tostring(err_call))\n\
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
        .expect("non-visible call/list inside an arm must error not-found");
    assert_eq!(out, "alpha:rejected");
}

/// Recursion depth accumulates across a fanout boundary: a section at the
/// call cap cannot fan out, because its arms would run one level deeper
/// still. Mutual `call` recursion drives the depth to the cap (a worker's
/// visible set never contains the worker, so only a call chain can reach
/// the cap); the store counter switches the last recursion step to `fanout`,
/// which must trip the same cap at the boundary rather than resetting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_recursion_across_the_boundary_trips_the_depth_cap() {
    // A runs at depths 0, 2, 4, 6, 8 (its 5th run); the fanout there would
    // spawn arms at depth 9, past MAX_CALL_DEPTH (8).
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
local ok, v = pcall(store.read, 'n.txt')\n\
local n = tonumber(ok and v or '0') + 1\n\
store.write('n.txt', tostring(n))\n\
if n >= 5 then\n\
  return fanout('## W', {'x'})\n\
end\n\
return call('## B')\n\
```\n\n\
## B\n\n\
```lua\n\
return call('## A')\n\
```\n\n\
## W\n\n\
---\n\n\
```lua\n\
return item\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a fanout at the call cap must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("fanout recursion exceeded cap of 8"),
        "expected the fanout recursion-cap error, got: {rendered}"
    );
}

/// An arm runs one call level deeper than its fanout caller: an arm
/// spawned at the cap's edge trips MAX_CALL_DEPTH on its OWN `call`.
/// Dropping the `+ 1` from the arm's depth in `run_fanout_arms` must fail
/// this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_inside_an_arm_spawned_near_the_cap_trips_the_depth_cap() {
    // B runs at depths 1, 3, 5, 7 (its 4th run); the fanout there spawns arms
    // at depth 8, and the arm's own `call` would need depth 9 - past
    // MAX_CALL_DEPTH (8).
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
return call('## B')\n\
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
return call('## A')\n\
```\n\n\
## W\n\n\
---\n\n\
```lua\n\
return call('## C')\n\
```\n\n\
## C\n\n\
---\n\n\
```lua\n\
return 'c-reply'\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a call inside an arm spawned at the cap's edge must fail");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("call recursion exceeded cap of 8"),
        "expected the call recursion-cap error, got: {rendered}"
    );
}

/// A worker that produces no output yields an honest empty
/// text - "done" would be a lie - and the arm still reports ok.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_without_output_and_no_incoming_reply_yields_empty_text() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha'})\n\
assert(r[1].ok, 'the arm succeeded')\n\
assert(r[1].text == '', 'a no-reply arm yields empty text')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\n\
assert(item == 'alpha')\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a no-reply arm must succeed with empty text");
    assert_eq!(out, "ok");
}
