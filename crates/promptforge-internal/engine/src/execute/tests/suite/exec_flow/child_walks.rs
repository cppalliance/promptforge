//! Off-walk sections and child-level walks: what a jump or `call` into a
//! child level runs, and the visible set a running child addresses.

use super::*;

/// A jump addresses an off-walk section directly, so it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_off_walk_section_runs_it() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\njump('## B')\n```\n\n\
## B\n\n\
---\n\n\
```lua\nreturn 'b-ran'\n```\n\n\
## C\n\n\
```lua\nreturn 'c-ran'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a jump to an off-walk section must run it");
    assert_eq!(out, "b-ran");
}

/// A jump to an H3 child starts a child-level walk at the target: it falls
/// through to the target's following siblings under the same rules as the
/// top-level walk, and when the level exhausts the parent walk resumes after
/// the jumper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_a_child_starts_the_child_level_walk() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a jump to a child must start the child-level walk");
    assert_eq!(out, "A\nX\nY\nB\n");
}

/// The child-level rule recurses: a jump from an H3 child to an H4 grandchild
/// starts an H4-level walk, and each level's exhaustion resumes its parent
/// after the jumper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_walk_recurses_to_h4() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the child-level rule must recurse to H4");
    assert_eq!(out, "A\nX\nP\nQ\nY\nB\n");
}

/// An off-walk child stays addressable: a jump to it runs it (and the
/// fall-through that follows skips nothing addressed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_to_an_off_walk_child_runs_it() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a jump to an off-walk child must run it");
    assert_eq!(out, "Off\nY\n");
}

/// `call` to a child starts a contained chain at the target: the chain
/// falls through to the target's following siblings under the same rules as
/// any walk, and the chain's final reply is the call's return value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_to_a_child_starts_a_contained_chain() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local r = call('### Sub')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("call to a child must start a contained chain");
    assert_eq!(out, "got:after-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

/// The top-level walk never descends: a section's children do not run unless
/// addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_never_descends_into_children() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("the walk must never descend into children");
    assert_eq!(out, "A\nB\n");
}

/// A running child's visible set is its own siblings plus its own children:
/// it can execute a child and jump to a sibling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_child_addresses_its_own_siblings_and_children() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
jump('### X')\n\
```\n\n\
### X\n\n\
```lua\n\
local r = call('#### Grand')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("a running child must address its own siblings and children");
    assert_eq!(out, "X:grand-ran\nY\n");
}

/// A running child cannot address a top-level section: the parent level is
/// not in its visible set, so the jump resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_child_cannot_address_a_top_level_section() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
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
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
jump('### Niece')\n\
```\n\n\
## B\n\n\
### Niece\n\n\
```lua\n\
return 'niece-ran'\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a jump to a niece must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

/// `sys.id` counts the sections the walk chain has entered: the detour into
/// a child level is the same chain, so it continues the count rather than
/// restarting it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_id_counts_the_sections_one_chain_enters_across_a_jump_into_a_child_level() {
    let md = flow_prompt!(
        "\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("sys.id must count the sections the one chain enters");
    assert_eq!(out, "0.1\n0.2\n0.3\n0.4\n");
}

/// `call()` on a child heading resolves the target's index within the
/// caller's CHILD slice, not the sibling slice: earlier children do not run,
/// and the chain falls through to the target's following child siblings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_to_a_later_child_runs_the_child_slice_from_that_index() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local r = call('### Sub2')\n\
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
```\n"
    );
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent())
        .await
        .expect("call to a later child must run the child slice from that index");
    assert_eq!(out, "got:sub3-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub2\nSub3\n");
}
