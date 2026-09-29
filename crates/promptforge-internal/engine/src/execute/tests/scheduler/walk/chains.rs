//! Contained chains: a `return` or `jump` inside a `call` or child walk
//! stays inside its chain, the outer walk never moves meanwhile, a jump
//! descent consumes no call depth, and a chain started past index 0 names
//! its start section before its first entry.

use super::*;
use crate::execute::run::Run;

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_child_walk_ends_the_whole_chain() {
    // A scalar return inside a jump-started child-level walk ends the whole
    // chain, not just the child level - the parent walk never resumes.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Return\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\nreturn 'x-value'\n```\n\n\
        ## B\n\n\
        ```lua\nerror('the return must end the chain before the parent resumes')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a return in the child walk ends the whole chain");

    assert_eq!(out, "x-value");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_call_is_contained_in_the_chain() {
    // A jump inside `call()` is contained by the chain - followed, not
    // rejected. The chain's index moves to the target, the sections between
    // the jumper and the target do not run, and the target's reply returns
    // to the caller.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Contained\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = call('## Sub')\n\
        return 'main:' .. r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Skipped\n\n\
        ```lua\nerror('the chain jump must move past me')\n```\n\n\
        ## Peer\n\n\
        ```lua\nreturn 'peer-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump inside call must be followed within the chain");

    assert_eq!(out, "main:peer-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_a_call_chain_moves_within_the_chain() {
    // A jump inside a `call()` chain to a sibling moves within the
    // contained chain - the walk continues from the jump target under the
    // normal rules, and the chain's final reply is the call's return value.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Move\n\n\
        ## A\n\n\
        ```lua\n\
        local r = call('## Sub')\n\
        return 'A:' .. r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Peer\n\n\
        ```lua\nstore.append('order.txt', 'Peer\\n')\n```\n\n\
        ## Tail\n\n\
        ```lua\n\
        store.append('order.txt', 'Tail\\n')\n\
        return 'tail-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump inside the chain must move within the chain");

    assert_eq!(out, "A:tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Peer\nTail\n");
}

#[tokio::test(flavor = "current_thread")]
async fn call_chain_jumps_to_a_child_and_returns_the_chain_result() {
    // The canonical contained chain: A calls Sub; Sub jumps to its child S1,
    // starting a child-level walk that falls through to S2; S2's return is
    // the chain's final text back to A, and the outer walk continues at B,
    // never having moved.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Chain\n\n\
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
        ```lua\nstore.append('order.txt', 'S1\\n')\n```\n\n\
        ### S2\n\n\
        ```lua\n\
        store.append('order.txt', 'S2\\n')\n\
        return 's2-result'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the call chain must jump, fall through, and return its final text");

    assert_eq!(out, "A1\nSub\nS1\nS2\nA2\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn the_outer_walk_never_moves_during_a_contained_chain() {
    // The outer walk never moves while a contained chain runs - wherever
    // the chain ends, the outer walk resumes at the section after the
    // caller.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Outer\n\n\
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
        ```lua\njump('## Peer')\n```\n\n\
        ## Peer\n\n\
        ```lua\n\
        store.append('order.txt', 'Peer\\n')\n\
        return 'p'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the outer walk must resume at the section after the caller");

    assert_eq!(out, "Peer\nA-done\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    // A return inside a contained chain ends the chain, not the run - the
    // returned value is the call's return, the chain's remaining sections
    // do not run, and the outer walk continues.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Scoped\n\n\
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
        ```lua\nreturn 'sub-reply'\n```\n\n\
        ## After\n\n\
        ```lua\nerror('a return must end the chain before fall-through')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a return must end the chain, not the run");

    assert_eq!(out, "A:sub-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn call_to_a_child_starts_a_contained_chain() {
    // `call` to a child starts a contained chain at the target - the chain
    // falls through to the target's following siblings under the same rules
    // as any walk, and the chain's final reply is the call's return value.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ChildExecute\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = call('### Sub')\n\
        return 'got:' .. r\n\
        ```\n\n\
        ### Sub\n\n\
        ```lua\nstore.append('order.txt', 'Sub\\n')\n```\n\n\
        ### After\n\n\
        ```lua\n\
        store.append('order.txt', 'After\\n')\n\
        return 'after-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("call to a child must start a contained chain");

    assert_eq!(out, "got:after-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_descent_does_not_consume_call_depth() {
    // The depth-cap interaction: a jump descent is not a call, so the child
    // level shares the chain's call-depth field. X and Y ping-pong calls
    // from inside a jump-started child walk; each entry appends once. The
    // cap trips when the ninth nested call would run (depth 9 > 8), after
    // exactly nine section entries - a descent that wrongly consumed depth
    // would trip the cap one entry earlier.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Main\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('depth.txt', 'x\\n')\n\
        return call('### Y')\n\
        ```\n\n\
        ### Y\n\n\
        ```lua\n\
        store.append('depth.txt', 'y\\n')\n\
        return call('### X')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "call recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
    assert_eq!(
        store.read("depth.txt").expect("depth log"),
        "x\ny\nx\ny\nx\ny\nx\ny\nx\n",
        "the descent shares the chain's call depth: nine entries, then the cap"
    );
}

#[test]
fn a_chain_started_past_index_0_names_its_start_section_before_its_first_entry() {
    // A jump out of H1, a `call`, and a spawn start their chain at the
    // target's index, so the chain's reports before its first entry name
    // the target, not the slice's first section. An index past the slice
    // names the prompt's title.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## A\n\n\
        ```lua\nreturn 'a'\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b'\n```\n";
    let prompt = parse(md);
    let (state, _host) = scheduler_context(&prompt);
    let mut run = Run::from_state(state);
    let scheduler = run.scheduler_for_test();

    assert_eq!(
        scheduler
            .name_before_first_entry_for_test(1)
            .expect("the chain starts"),
        "B",
        "a chain started at index 1 names the section at index 1"
    );
    assert_eq!(
        scheduler
            .name_before_first_entry_for_test(2)
            .expect("the chain starts"),
        "Title",
        "a chain started past the slice names the prompt's title"
    );
}
