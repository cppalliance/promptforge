//! Off-walk sections and child-level walks: what a jump or `call` into a
//! child level runs, and the visible set a running child addresses.

use super::*;

/// A jump addresses an off-walk section directly, so it runs.
#[test]
fn jump_to_off_walk_section_runs_it() {
    let source = PromptSource::new()
        .section("A", "jump('## B')")
        .prose("B", "---\n\n```lua\nreturn 'b-ran'\n```")
        .section("C", "return 'c-ran'");
    assert_eq!(Scenario::new(source.build()).run().text(), "b-ran");
}

/// A jump to an H3 child starts a child-level walk at the target: it falls
/// through to the target's following siblings under the same rules as the
/// top-level walk, and when the level exhausts the parent walk resumes after
/// the jumper.
#[test]
fn jump_to_a_child_starts_the_child_level_walk() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                store.append('order.txt', 'A\\n')
                jump('### X')",
            ),
            ("### X", "store.append('order.txt', 'X\\n')"),
            ("### Y", "store.append('order.txt', 'Y\\n')"),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "A\nX\nY\nB\n");
}

/// The child-level rule recurses: a jump from an H3 child to an H4 grandchild
/// starts an H4-level walk, and each level's exhaustion resumes its parent
/// after the jumper.
#[test]
fn child_walk_recurses_to_h4() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                store.append('order.txt', 'A\\n')
                jump('### X')",
            ),
            (
                "### X",
                "
                store.append('order.txt', 'X\\n')
                jump('#### P')",
            ),
            ("#### P", "store.append('order.txt', 'P\\n')"),
            ("#### Q", "store.append('order.txt', 'Q\\n')"),
            ("### Y", "store.append('order.txt', 'Y\\n')"),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "A\nX\nP\nQ\nY\nB\n");
}

/// An off-walk child stays addressable: a jump to it runs it (and the
/// fall-through that follows skips nothing addressed).
#[test]
fn jump_to_an_off_walk_child_runs_it() {
    let source = PromptSource::new()
        .section("A", "jump('### Off')")
        .section("### X", "store.append('order.txt', 'X\\n')")
        .prose(
            "### Off",
            "---\n\n```lua\nstore.append('order.txt', 'Off\\n')\n```",
        )
        .section("### Y", "store.append('order.txt', 'Y\\n')")
        .section("B", "return store.read('order.txt')");
    assert_eq!(Scenario::new(source.build()).run().text(), "Off\nY\n");
}

/// `call` to a child starts a contained chain at the target: the chain
/// falls through to the target's following siblings under the same rules as
/// any walk, and the chain's final reply is the call's return value.
#[test]
fn call_to_a_child_starts_a_contained_chain() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local r = call('### Sub')
                return 'got:' .. r",
            ),
            ("### Sub", "store.append('order.txt', 'Sub\\n')"),
            (
                "### After",
                "
                store.append('order.txt', 'After\\n')
                return 'after-reply'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "got:after-reply");
    assert_eq!(trace.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

/// The top-level walk never descends: a section's children do not run unless
/// addressed.
#[test]
fn walk_never_descends_into_children() {
    let source = prompt(
        "",
        &[
            ("A", "store.append('order.txt', 'A\\n')"),
            ("### Child", "error('a child must not run by fall-through')"),
            (
                "B",
                "
                store.append('order.txt', 'B\\n')
                return store.read('order.txt')",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "A\nB\n");
}

/// A running child's visible set is its own siblings plus its own children:
/// it can execute a child and jump to a sibling.
#[test]
fn running_child_addresses_its_own_siblings_and_children() {
    let source = prompt(
        "",
        &[
            ("A", "jump('### X')"),
            (
                "### X",
                "
                local r = call('#### Grand')
                store.append('order.txt', 'X:' .. r .. '\\n')
                jump('### Y')",
            ),
            ("#### Grand", "return 'grand-ran'"),
            ("### Y", "store.append('order.txt', 'Y\\n')"),
            ("B", "return store.read('order.txt')"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "X:grand-ran\nY\n");
}

/// A running child cannot address a top-level section: the parent level is
/// not in its visible set, so the jump resolves as not-found.
#[test]
fn running_child_cannot_address_a_top_level_section() {
    let source = prompt(
        "",
        &[
            ("A", "jump('### X')"),
            ("### X", "jump('## B')"),
            ("B", "return 'b-ran'"),
        ],
    );
    let trace = Scenario::new(source).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("not found"),
        "a top-level section is not in a child's visible set: {rendered}"
    );
}

/// A sibling's child (a niece or nephew) is not in the visible set: the jump
/// resolves as not-found.
#[test]
fn jump_to_a_niece_errors() {
    let source = PromptSource::new()
        .section("A", "jump('### Niece')")
        .prose("B", "")
        .section("### Niece", "return 'niece-ran'");
    let trace = Scenario::new(source.build()).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

/// `sys.id` counts the sections the walk chain has entered: the detour into
/// a child level is the same chain, so it continues the count rather than
/// restarting it.
#[test]
fn sys_id_counts_the_sections_one_chain_enters_across_a_jump_into_a_child_level() {
    let source = prompt(
        "",
        &[
            (
                "A",
                "
                store.append('ids.txt', tostring(sys.id) .. '\\n')
                jump('### X')",
            ),
            (
                "### X",
                "store.append('ids.txt', tostring(sys.id) .. '\\n')",
            ),
            (
                "### Y",
                "store.append('ids.txt', tostring(sys.id) .. '\\n')",
            ),
            (
                "B",
                "
                store.append('ids.txt', tostring(sys.id) .. '\\n')
                return store.read('ids.txt')",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "0.1\n0.2\n0.3\n0.4\n");
}

/// `call()` on a child heading resolves the target's index within the
/// caller's CHILD slice, not the sibling slice: earlier children do not run,
/// and the chain falls through to the target's following child siblings.
#[test]
fn call_to_a_later_child_runs_the_child_slice_from_that_index() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local r = call('### Sub2')
                return 'got:' .. r",
            ),
            ("### Sub1", "error('an earlier child must not run')"),
            ("### Sub2", "store.append('order.txt', 'Sub2\\n')"),
            (
                "### Sub3",
                "
                store.append('order.txt', 'Sub3\\n')
                return 'sub3-reply'",
            ),
        ],
    );
    let trace = Scenario::new(source).run();
    assert_eq!(trace.text(), "got:sub3-reply");
    assert_eq!(trace.read("order.txt").expect("order log"), "Sub2\nSub3\n");
}
