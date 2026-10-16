//! Control flow inside a fanout arm: `prompt.list`, `call`, nested
//! `fanout`, and `jump` over the worker's visible set, the depth cap
//! across the arm boundary, and an arm's text when it replies nothing.

use super::super::super::arm_fanout;
use super::*;

/// `prompt.list` inside a fanout arm reads a list section's items: the arm
/// resolves like any section (the worker's siblings minus the worker, plus
/// its children).
#[test]
fn prompt_list_inside_a_fanout_arm_reads_items() {
    let source = arm_fanout()
        .section(
            "### Worker",
            "
            local items = prompt.list('### Items')
            return item .. ':' .. table.concat(items, ',')",
        )
        .prose("### Items", "- x\n- y");
    assert_eq!(Scenario::new(source.build()).run().text(), "alpha:x,y");
}

/// `call` inside a fanout arm runs a contained chain over the worker's
/// visible set: the chain is the arm's child (`0.0.0` under arm `0.0`), runs
/// as plain
/// sections (no `item` seed), and its final reply is the call's return value.
/// The arm and the contained chain also see the run's `sys.section_count`.
/// The run's limit is 1 and the contained chain fans out: while the chain
/// waits on its arms it gives back the slot its arm holds, so its arms can
/// run instead of queueing behind that slot forever. The driver fails a
/// stalled run, and nextest's timeout fails a hung one.
#[test]
fn call_inside_a_fanout_arm_runs_a_contained_chain() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                tasks.concurrency(1)
                local r = tasks.fanout('### Worker', {'alpha'})
                return r[1].text",
            ),
            (
                "### Worker",
                "
                assert(sys.section_count == 1, 'the arm sees the run top-level section count')
                local got = call('### Sub')
                return 'worker:' .. got .. ':' .. item",
            ),
            (
                "### Sub",
                "
                assert(sys.id == '0.0.0.0', 'a contained chain is the arm chain child and starts at entry 0')
                assert(item == nil, 'a contained chain runs as a plain section')
                assert(sys.section_count == 1, 'a contained chain sees the run section count')
                local leaves = tasks.fanout('#### Leaf', {'x', 'y'})
                var.leaves = leaves[1].text .. leaves[2].text",
            ),
            ("#### Leaf", "return item .. '!'"),
            ("### Tail", "return 'tail-reply:' .. var.leaves"),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "worker:tail-reply:x!y!:alpha");
}

/// `fanout` inside a fanout arm maps over a collection: the nested worker
/// resolves over the outer worker's visible set, and the nested structured
/// results come back to the outer arm.
#[test]
fn fanout_inside_a_fanout_arm_maps_over_a_collection() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Outer', {'a', 'b'})
                return table.concat({r[1].text, r[2].text}, ';')",
            ),
            (
                "### Outer",
                "
                local inner = tasks.fanout('### Inner', {item .. '1', item .. '2'})
                assert(inner[1].ok and inner[2].ok)
                return item .. ':' .. inner[1].text .. ',' .. inner[2].text",
            ),
            (
                "### Inner",
                "
                assert(tostring(sys.index) == string.sub(item, -1), 'a nested fanout restarts sys.index at 1')
                return item .. '!'",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "a:a1!,a2!;b:b1!,b2!");
}

/// A jump inside a fanout arm transfers control: the arm's remaining blocks
/// are skipped, a child walk runs from the target under the Engine's
/// chain-slice rule (continuing the arm chain's `sys.id` sequence, falling
/// through to the
/// target's following siblings), and the child walk's reply becomes the arm's
/// text.
#[test]
fn jump_inside_a_fanout_arm_drives_a_child_walk() {
    let source = arm_fanout()
        .section(
            "### Worker",
            "
            jump('### Target')
            error('the arm remaining blocks are skipped')",
        )
        .section(
            "### Target",
            "
            assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')
            store.append('order.txt', 'Target\\n')",
        )
        .section(
            "### Tail",
            "
            store.append('order.txt', 'Tail\\n')
            return 'tail-reply'",
        );
    let trace = Scenario::new(source.build()).run();
    assert_eq!(trace.text(), "tail-reply");
    assert_eq!(
        trace.read("order.txt").expect("order log"),
        "Target\nTail\n",
        "the child walk runs the target and falls through to its siblings"
    );
}

/// A jump from an arm into one of the worker's own children drives the
/// child-level walk over the worker's child slice: the target takes the arm
/// chain's next `sys.id` and no `item` seed, and the walk falls
/// through to the target's child siblings.
#[test]
fn jump_inside_a_fanout_arm_to_a_worker_child_walks_the_child_slice() {
    let source = arm_fanout()
        .section(
            "### Worker",
            "
            jump('#### Child')
            error('the arm remaining blocks are skipped')",
        )
        .section(
            "#### Child",
            "
            assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')
            assert(item == nil, 'the child walk runs as a plain section')
            store.append('order.txt', 'Child\\n')",
        )
        .section(
            "#### ChildTail",
            "
            store.append('order.txt', 'ChildTail\\n')
            return 'child-tail-reply'",
        );
    let trace = Scenario::new(source.build()).run();
    assert_eq!(trace.text(), "child-tail-reply");
    assert_eq!(
        trace.read("order.txt").expect("order log"),
        "Child\nChildTail\n",
        "the child walk runs the target and falls through to its child siblings"
    );
}

/// A jump-started child walk that produces no return and no reply exhausts:
/// the arm's text is the empty string.
#[test]
fn jump_inside_a_fanout_arm_to_a_silent_chain_returns_empty_text() {
    let source = arm_fanout()
        .section("### Worker", "jump('### Target')")
        .section("### Target", "store.append('order.txt', 'Target\\n')");
    let trace = Scenario::new(source.build()).run();
    assert_eq!(
        trace.text(),
        "",
        "an exhausted child walk with no reply maps to empty text"
    );
    assert_eq!(trace.read("order.txt").expect("order log"), "Target\n");
}

/// A jump from an arm to a heading outside the worker's visible set (the
/// top-level parent) resolves as not-found, and the error lists only the
/// worker's visible set.
#[test]
fn jump_inside_a_fanout_arm_to_a_non_visible_heading_errors() {
    let source = arm_fanout()
        .section("### Worker", "jump('## Parent')")
        .prose("### Items", "- x");
    let trace = Scenario::new(source.build()).run();
    let rendered = format!("{:?}", trace.error());
    assert!(
        rendered.contains("not found"),
        "a non-visible heading must not resolve: {rendered}"
    );
    assert!(
        rendered.contains("### Items"),
        "the error lists the worker's visible set: {rendered}"
    );
}

/// `call` and `prompt.list` inside an arm naming a section outside the
/// worker's visible set both reject it: `call` errors not-found, which the
/// arm catches with `pcall` and asserts on, and `prompt.list` reads `nil`.
#[test]
fn call_raises_and_list_reads_nil_for_non_visible_sections_in_a_fanout_arm() {
    let source = arm_fanout()
        .section(
            "### Worker",
            "
            local ok_call, err_call = pcall(call, '## Parent')
            assert(not ok_call, 'call of a non-visible section must fail')
            assert(tostring(err_call):find('not found', 1, true), 'call error must be not-found: ' .. tostring(err_call))
            assert(prompt.list('## Parent') == nil, 'prompt.list of a non-visible section must read nil')
            return item .. ':rejected'",
        )
        .prose("### Items", "- x");
    assert_eq!(Scenario::new(source.build()).run().text(), "alpha:rejected");
}

/// Recursion depth accumulates across a fanout boundary: a section at the
/// call cap cannot fan out, because its arms would run one level deeper
/// still. Mutual `call` recursion drives the depth to the cap (a worker's
/// visible set never contains the worker, so only a call chain can reach
/// the cap); the store counter switches the last recursion step to `fanout`,
/// which must trip the same cap at the boundary rather than resetting.
#[test]
fn fanout_recursion_across_the_boundary_trips_the_depth_cap() {
    // A runs at depths 0, 2, 4, 6, 8 (its 5th run); the fanout there would
    // spawn arms at depth 9, past MAX_CALL_DEPTH (8).
    let source = PromptSource::new()
        .section(
            "A",
            "
            local ok, v = pcall(store.read, 'n.txt')
            local n = tonumber(ok and v or '0') + 1
            store.write('n.txt', tostring(n))
            if n >= 5 then
              return tasks.fanout('## W', {'x'})
            end
            return call('## B')",
        )
        .section("B", "return call('## A')")
        .prose("W", "---\n\n```lua\nreturn item\n```");
    let trace = Scenario::new(source.build()).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("tasks.fanout recursion exceeded cap of 8"),
        "expected the fanout recursion-cap error, got: {rendered}"
    );
}

/// An arm runs one call level deeper than its fanout caller: an arm
/// spawned at the cap's edge trips MAX_CALL_DEPTH on its OWN `call`.
/// Dropping the `+ 1` from the arm's depth in `run_fanout_arms` must fail
/// this test.
#[test]
fn call_inside_an_arm_spawned_near_the_cap_trips_the_depth_cap() {
    // B runs at depths 1, 3, 5, 7 (its 4th run); the fanout there spawns arms
    // at depth 8, and the arm's own `call` would need depth 9 - past
    // MAX_CALL_DEPTH (8).
    let source = PromptSource::new()
        .section("A", "return call('## B')")
        .section(
            "B",
            "
            local ok, v = pcall(store.read, 'n.txt')
            local n = tonumber(ok and v or '0') + 1
            store.write('n.txt', tostring(n))
            if n >= 4 then
              local r = tasks.fanout('## W', {'x'})
              return r[1].text
            end
            return call('## A')",
        )
        .prose("W", "---\n\n```lua\nreturn call('## C')\n```")
        .prose("C", "---\n\n```lua\nreturn 'c-reply'\n```");
    let trace = Scenario::new(source.build()).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("call recursion exceeded cap of 8"),
        "expected the call recursion-cap error, got: {rendered}"
    );
}

/// A worker that produces no output yields an honest empty
/// text - "done" would be a lie - and the arm still reports ok.
#[test]
fn fanout_arm_without_output_and_no_incoming_reply_yields_empty_text() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Worker', {'alpha'})
                assert(r[1].ok, 'the arm succeeded')
                assert(r[1].text == '', 'a no-reply arm yields empty text')
                return 'ok'",
            ),
            ("### Worker", "assert(item == 'alpha')"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}
