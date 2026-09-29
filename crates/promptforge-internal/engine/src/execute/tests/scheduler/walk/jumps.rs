//! Jumps and observation boundaries: a jump skips the jumper's remaining
//! blocks, cannot target its own section, runs an off-walk target, and
//! keeps `var`; the jumper finishes before its target starts, even when
//! the target does not resolve, and an erroring section reports started
//! but not finished.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn jump_transfer_skips_the_jumpers_remaining_blocks() {
    // The jump transfers control and the jumper's remaining blocks never
    // run.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
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
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("jump must transfer control");

    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

#[tokio::test(flavor = "current_thread")]
async fn section_cannot_jump_to_itself() {
    // The caller is outside its own visible set, so naming its own heading
    // to `jump` resolves as not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Self\n\n\
        ## Self\n\n\
        ```lua\njump('## Self')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("self-jump must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_off_walk_section_runs_it() {
    // A jump addresses an off-walk section directly, so it runs.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Addressed\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nreturn 'b-ran'\n```\n\n\
        ## C\n\n\
        ```lua\nreturn 'c-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump to an off-walk section must run it");

    assert_eq!(out, "b-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_a_jump() {
    // The jumper's `var` writes cross the transfer, and the target's
    // writes roll forward into the fall-through that follows.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
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
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("var must persist across the jump");

    assert_eq!(out, "ac");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_fires_section_finished_for_the_jumper_before_the_target_starts() {
    // The jump half of the observation-boundary contract (the fall-through
    // half is `fall_through_fires_section_finished_before_the_next_section_starts`
    // above): a jump is a completion, so the jumper's armed frame drop
    // fires SECTION_FINISHED before the target's SECTION_STARTED.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Boundaries\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the jump completes both sections");

    assert_eq!(out, "b-ran");
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    let boundaries: Vec<(String, String)> = recorder
        .events()
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

#[tokio::test(flavor = "current_thread")]
async fn an_erroring_section_reports_started_but_not_finished() {
    // A section that errors mid-walk emits SECTION_STARTED and never
    // SECTION_FINISHED - the frame's drop stays unarmed on the error path.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fail\n\n\
        ## Only\n\n\
        ```lua\nerror('expected failure')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = TokioDriver::new(&ctx, host, None).drive().await;

    assert!(result.is_err());
    let observed = recorder.events();
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

#[tokio::test(flavor = "current_thread")]
async fn a_failed_jump_resolution_still_finishes_the_jumper() {
    // The error half of the jump observation boundary: the jumper's frame
    // closes as completed before the heading resolves, so SECTION_FINISHED
    // fires for the jumper even when the target does not resolve.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Unresolved\n\n\
        ## A\n\n\
        ```lua\njump('## Missing')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = TokioDriver::new(&ctx, host, None).drive().await;

    let error = result.expect_err("an unresolvable jump target must fail the run");
    assert!(
        error.to_string().contains("not found"),
        "the failure is the resolution, not the transfer: {error}"
    );
    let observed = recorder.events();
    assert!(
        observed.contains(&("A".to_string(), detail::SECTION_FINISHED.to_string())),
        "the jumper completed before the resolution failed: {observed:?}"
    );
}
