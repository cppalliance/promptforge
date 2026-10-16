//! Contained `call` chains and the jumps inside them, the `sys.id` and
//! `sys.taskid` values chains and fanout arms take, and the recursion and
//! self-address limits.

use super::*;
use crate::RunErrorKind;

/// A jump transfers control and the jumper's remaining blocks never run.
#[test]
fn jump_transfer_skips_the_jumpers_remaining_blocks() {
    let source = prompt(
        "",
        &[
            (
                "Check",
                "
                store.write('seen.txt', 'check')
                jump('## Help')
                store.write('seen.txt', 'should-not-run')",
            ),
            ("Accept", "return 'accepted'"),
            ("Help", "return 'helped:' .. store.read('seen.txt')"),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "helped:check");
    assert_eq!(trace.read("seen.txt").expect("seen"), "check");
}

/// A jump inside `call()` is contained by the chain: followed, not
/// rejected. The chain's index moves to the target - the sections between
/// the jumper and the target do not run - and the target's reply returns to
/// the caller.
#[test]
fn jump_inside_call_is_contained_in_the_chain() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local r = call('## Sub')
                return 'main:' .. r",
            ),
            ("Sub", "jump('## Peer')"),
            ("Skipped", "error('the chain jump must move past me')"),
            ("Peer", "return 'peer-ran'"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "main:peer-ran");
}

/// The canonical contained chain (decision 14): A calls Sub; Sub jumps to
/// its child S1, starting a child-level chain that falls through to S2; S2's
/// return is the chain's final text back to A, and the outer walk
/// continues at B, never having moved.
#[test]
fn call_chain_jumps_to_a_child_and_returns_the_chain_result() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                store.append('order.txt', 'A1\\n')
                local r = call('## Sub')
                assert(r == 's2-result', 'the chain final text returns to A')
                store.append('order.txt', 'A2\\n')",
            ),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
            (
                "Sub",
                "
                store.append('order.txt', 'Sub\\n')
                jump('### S1')",
            ),
            ("### S1", "store.append('order.txt', 'S1\\n')"),
            (
                "### S2",
                "
                store.append('order.txt', 'S2\\n')
                return 's2-result'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "A1\nSub\nS1\nS2\nA2\nB\n");
}

/// The second canonical example (decision 14): A executes the off-walk S1,
/// which runs because it is addressed; the chain falls through to S2, and
/// S2's reply returns to A. The main walk ends at B and never runs S1 or S2.
#[test]
fn call_chain_over_off_walk_siblings_returns_to_the_caller() {
    let source = PromptSource::new()
        .section(
            "A",
            "
            local r = call('## S1')
            store.append('order.txt', 'A:' .. r .. '\\n')",
        )
        .section(
            "B",
            "
            store.append('order.txt', 'B\\n')
            return store.read('order.txt')",
        )
        .prose(
            "S1",
            "
            ---

            ```lua
            store.append('order.txt', 'S1\\n')
            ```",
        )
        .section(
            "S2",
            "
            store.append('order.txt', 'S2\\n')
            return 's2-reply'",
        );
    let trace = Scenario::new(source.build()).run();
    assert_eq!(trace.text(), "S1\nS2\nA:s2-reply\nB\n");
}

/// A jump inside a `call()` chain to a sibling moves within the
/// contained chain: the walk continues from the jump target under the normal
/// rules, and the chain's final reply is the call's return value.
#[test]
fn jump_inside_a_call_chain_moves_within_the_chain() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                local r = call('## Sub')
                return 'A:' .. r",
            ),
            ("Sub", "jump('## Peer')"),
            ("Peer", "store.append('order.txt', 'Peer\\n')"),
            (
                "Tail",
                "
                store.append('order.txt', 'Tail\\n')
                return 'tail-reply'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "A:tail-reply");
    assert_eq!(trace.read("order.txt").expect("order log"), "Peer\nTail\n");
}

/// The outer walk never moves while a contained chain runs: wherever the
/// chain ends, the outer walk resumes at the section after the caller.
#[test]
fn the_outer_walk_never_moves_during_a_contained_chain() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                call('## Sub')
                store.append('order.txt', 'A-done\\n')",
            ),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
            ("Sub", "jump('## Peer')"),
            (
                "Peer",
                "
                store.append('order.txt', 'Peer\\n')
                return 'p'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "Peer\nA-done\nB\n");
}

/// A return inside a contained chain ends the chain, not the run: the
/// returned value is the call's return, the chain's remaining sections do
/// not run, and the outer walk continues.
#[test]
fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                local r = call('## Sub')
                store.append('order.txt', 'A:' .. r .. '\\n')",
            ),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
            ("Sub", "return 'sub-reply'"),
            (
                "After",
                "error('a return must end the chain before fall-through')",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "A:sub-reply\nB\n");
}

/// A `call` chain's sections take ids nested under the chain's own id: the
/// contained chain is the walk's child `0.0`, so its entries are `0.0.N`,
/// and the outer walk resumes its own `0.N` sequence when the chain ends.
#[test]
fn a_call_chain_counts_its_own_entries_and_the_outer_walk_resumes_its_own_sequence() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                assert(sys.id == '0.1', 'the first walked section takes entry 1 of the root chain')
                local r = call('## Sub')
                store.append('order.txt', r .. '\\n')",
            ),
            (
                "B",
                "
                assert(sys.id == '0.2', 'the outer walk resumes its own sequence')
                return store.read('order.txt')",
            ),
            (
                "Sub",
                "assert(sys.id == '0.0.0', 'the contained chain is child 0 and starts at entry 0')",
            ),
            (
                "Tail",
                "
                assert(sys.id == '0.0.1', 'the chain fall-through takes its next entry')
                return 'tail-reply'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "tail-reply\n");
}

/// Entering the same section twice hands out two distinct `sys.id` values:
/// two call children of the walk are two chains.
#[test]
fn entering_the_same_section_twice_takes_two_ids() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local a = call('## Sub')
                local b = call('## Sub')
                return a .. ',' .. b",
            ),
            ("Sub", "return tostring(sys.id)"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "0.0.0,0.1.0");
}

/// Fanout arms take unique `sys.id` values nested under the caller's chain
/// (each arm is a child chain, allocated in collection order at dispatch,
/// so the ids never depend on finish order) and a per-fanout 1-based
/// `sys.index`.
#[test]
fn fanout_arms_take_child_ids_in_collection_order_and_a_per_fanout_index() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Worker', {'a', 'b'})
                local function parts(s) return string.match(s, '^(%d+):([%d%.]+)$') end
                local i1, id1 = parts(r[1].text)
                local i2, id2 = parts(r[2].text)
                assert(i1 == '1' and i2 == '2', 'sys.index is the 1-based per-fanout position')
                assert(id1 ~= id2, 'arms take unique ids')
                assert(id1 == '0.0.0' and id2 == '0.1.0', 'arm ids are the caller children in collection order')
                return 'ok'",
            ),
            (
                "### Worker",
                "return tostring(sys.index) .. ':' .. tostring(sys.id)",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

/// `sys.index` exists only inside a fanout arm; reading it in a walked
/// section raises the sealed-sys unknown-field error.
#[test]
fn sys_index_outside_a_fanout_errors() {
    let trace = Scenario::new(prompt("", &[("Main", "return tostring(sys.index)")])).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("unknown sys field 'index'"),
        "sys.index is fanout-only: {rendered}"
    );
}

/// `sys.taskid` is the nearest enclosing task: a fanout arm is a task the
/// `fanout` shim spawns, so it reports its own id - the caller's first
/// child - while the main walk stays task `0`.
#[test]
fn sys_taskid_inside_a_fanout_arm_is_the_arms_own_task() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                assert(sys.taskid == '0', 'the main walk is task 0, got ' .. sys.taskid)
                local results = tasks.fanout('### Worker', {'a'})
                return results[1].text",
            ),
            ("### Worker", "return sys.taskid"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "0.0");
}

/// Nested `call()` is capped at [`MAX_CALL_DEPTH`]. Locks the
/// `call_depth` divergence threaded through the one shared walk (the
/// top-level walk always enters at depth 0; the subroutine keeps its depth).
/// The caller is not in its own visible set, so the recursion is mutual.
#[test]
fn call_recursion_is_capped() {
    let source = prompt(
        "",
        &[("A", "return call('## B')"), ("B", "return call('## A')")],
    );
    let trace = Scenario::new(source).run();
    let error = trace.error();
    assert_eq!(
        error.kind(),
        RunErrorKind::Quota,
        "the cap is a quota: {error}"
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("call recursion exceeded cap of 8"),
        "expected the call recursion-cap error, got: {rendered}"
    );
}

/// The caller is outside its own visible set (decision 3): naming its own
/// heading to `call` resolves as not-found.
#[test]
fn section_cannot_call_itself() {
    let trace = Scenario::new(prompt("", &[("Self", "return call('## Self')")])).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

/// The caller is outside its own visible set (decision 3): naming its own
/// heading to `jump` resolves as not-found.
#[test]
fn section_cannot_jump_to_itself() {
    let trace = Scenario::new(prompt("", &[("Self", "jump('## Self')")])).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}
