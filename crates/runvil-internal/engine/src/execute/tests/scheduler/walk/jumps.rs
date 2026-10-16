//! Jumps and observation boundaries: a jump skips the jumper's remaining
//! blocks, cannot target its own section, runs an off-walk target, and
//! keeps `var`; the jumper finishes before its target starts, even when
//! the target does not resolve, and an erroring section reports started
//! but not finished.

use super::*;
use crate::execute::tests::context::sequence;

#[test]
fn jump_transfer_skips_the_jumpers_remaining_blocks() {
    // The jump transfers control and the jumper's remaining blocks never
    // run.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Jump\n\n\
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
        ```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "helped:check");
    assert_eq!(trace.read("seen.txt").expect("seen"), "check");
}

#[test]
fn section_cannot_jump_to_itself() {
    // The caller is outside its own visible set, so naming its own heading
    // to `jump` resolves as not-found.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Self\n\n\
        ## Self\n\n\
        ```lua\njump('## Self')\n```\n";
    let trace = scheduler_scenario(md).run();

    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

#[test]
fn jump_to_off_walk_section_runs_it() {
    // A jump addresses an off-walk section directly, so it runs.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Addressed\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nreturn 'b-ran'\n```\n\n\
        ## C\n\n\
        ```lua\nreturn 'c-ran'\n```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "b-ran");
}

#[test]
fn var_persists_across_a_jump() {
    // The jumper's `var` writes cross the transfer, and the target's
    // writes roll forward into the fall-through that follows.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Var\n\n\
        ## A\n\n\
        ```lua\n\
        var.from_a = 'a'\n\
        jump('## C')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\nerror('the jump must skip B')\n```\n\n\
        ## C\n\n\
        ```lua\n\
        assert(var.from_a == 'a', 'the jump keeps the jumper writes')\n\
        var.from_c = 'c'\n\
        ```\n\n\
        ## D\n\n\
        ```lua\n\
        assert(var.from_c == 'c', 'fall-through after the jumped target keeps var')\n\
        return var.from_a .. var.from_c\n\
        ```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "ac");
}

#[test]
fn a_jump_fires_section_finished_for_the_jumper_before_the_target_starts() {
    // The jump half of the observation-boundary contract (the fall-through
    // half is `fall_through_fires_section_finished_before_the_next_section_starts`
    // above): a jump is a completion, so the jumper's armed frame drop
    // fires SECTION_FINISHED before the target's SECTION_STARTED.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Boundaries\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "b-ran");
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    let boundaries: Vec<(String, String)> = sequence(&trace)
        .into_iter()
        .filter(|(_, event)| event == &started || event == &finished)
        .collect();
    assert_eq!(
        boundaries,
        vec![
            ("A".to_owned(), started.clone()),
            ("A".to_owned(), finished.clone()),
            ("B".to_owned(), started.clone()),
            ("B".to_owned(), finished.clone()),
        ]
    );
}

#[test]
fn an_erroring_section_reports_started_but_not_finished() {
    // A section that errors mid-walk emits SECTION_STARTED and never
    // SECTION_FINISHED - the frame's drop stays unarmed on the error path.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Fail\n\n\
        ## Only\n\n\
        ```lua\nerror('expected failure')\n```\n";
    let trace = scheduler_scenario(md).run();

    assert!(
        matches!(trace.result(), RunResult::Failure(_)),
        "the erroring section fails the run, got {:?}",
        trace.result()
    );
    let observed = sequence(&trace);
    assert!(
        observed.contains(&("Only".to_string(), detail::SECTION_STARTED.to_string())),
        "the erroring section must report started: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(_, event)| event == &detail::SECTION_FINISHED.to_string()),
        "the erroring section must never report finished: {observed:?}"
    );
}

#[test]
fn a_failed_jump_resolution_still_finishes_the_jumper() {
    // The error half of the jump observation boundary: the jumper's frame
    // closes as completed before the heading resolves, so SECTION_FINISHED
    // fires for the jumper even when the target does not resolve.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Unresolved\n\n\
        ## A\n\n\
        ```lua\njump('## Missing')\n```\n";
    let trace = scheduler_scenario(md).run();

    let error = trace.error();
    assert!(
        error.to_string().contains("not found"),
        "the failure is the resolution, not the transfer: {error}"
    );
    let observed = sequence(&trace);
    assert!(
        observed.contains(&("A".to_string(), detail::SECTION_FINISHED.to_string())),
        "the jumper completed before the resolution failed: {observed:?}"
    );
}
