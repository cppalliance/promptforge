//! Fanout failure semantics on the scheduler. The arms are task chains the
//! `fanout` shim spawns, so their lifecycle reports through the `Task*`
//! observations: started under the caller's section, the terminal under the
//! worker's.

use super::super::models_loop::{echo_tools, loop_context_observed};
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn fanout_empty_collection_errors_before_any_scheduling() {
    // Pin of the pre-scheduling guard: the fanout errors before any arm is
    // created - no STARTED observation, and the worker's store tripwire
    // never fires.
    let store = TestStore::new();
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {})\n```\n\n\
        ### Worker\n\n\
        ```lua\nstore.write('ran.txt', 'yes')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, recorder.clone());
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("an empty collection must error");

    assert!(
        error.to_string().contains("empty collection"),
        "error was: {error}"
    );
    assert!(
        store.read("ran.txt").is_err(),
        "the worker never ran: the rejection precedes scheduling"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        0,
        "no arm was ever started: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_worker_that_is_a_list_section_errors() {
    // Pin of the worker-template guard: a resolved list section is not a
    // worker template.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Items', {'x'})\n```\n\n\
        ### Items\n\n\
        - a\n\
        - b\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a list section is not a worker template");

    assert!(
        error
            .to_string()
            .contains("is a list section, not a worker template"),
        "error was: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_depth_cap_reads_the_chain_field() {
    // Pin of the fanout depth-cap guard: Alpha and Beta ping-pong calls
    // through nested call chains, and the chain that lands at depth 8 calls
    // fanout - each arm would run one level deeper, so the cap fires from
    // the requesting chain's call-depth field. The arm's spawn sets the
    // fanout mark, so the spawn arm names the cap after `fanout` in the
    // typed error itself; the shim re-raises that table and the retained
    // typed `Lua` error is substituted back at every `call` level, so the
    // run's error is the typed error itself: the exact variant, the exact
    // text, no runtime-error prefix or traceback.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\n\
        var.n = (var.n or 0) + 1\n\
        if var.n >= 9 then return fanout('### Worker', {'x'}) end\n\
        return call('## Beta')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn item\n```\n\n\
        ## Beta\n\n\
        ```lua\n\
        var.n = var.n + 1\n\
        return call('## Alpha')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("the fanout depth cap must fail the run");

    assert!(
        matches!(&error, Error::Lua(message) if message == "fanout recursion exceeded cap of 8"),
        "expected the typed Lua depth-cap error with fanout's own wording, got {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "fanout recursion exceeded cap of 8",
        "the rendered text is exactly the fanout cap message: no call wording, no prefix"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_exhausted_arm_becomes_the_incomplete_stub_and_its_sibling_still_lands() {
    // The `tool_loop_exhausted` arm rule: one arm's `models.loop` runs its
    // round cap against a never-converging model and fails as
    // `tool_loop_exhausted`; the shim turns that arm's slot into the
    // incomplete stub (`ok = false`, `exhausted = true`) and the fanout
    // continues, so the sibling's plain result lands beside it. Only the
    // looping arm touches the gateway, so the scripted replies serve one
    // arm and the run is deterministic. The exhausted arm still reports
    // `TaskFailed` (the stub is the fanout's recovery, not the arm's), and
    // the sibling `TaskSucceeded`.
    let cap = 2;
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_0", "echo", "{\"value\":\"x\"}"),
        resp_tool_call("call_1", "echo", "{\"value\":\"x\"}"),
    ])
    .await;
    let md = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\nmax_tool_iterations: {cap}\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {{'loop', 'plain'}})\n\
        assert(#r == 2, 'both slots are filled')\n\
        assert(r[1].ok == false and r[1].exhausted == true, 'the exhausted arm is flagged')\n\
        assert(r[1].item == 'loop', 'the stub keeps its item')\n\
        assert(r[2].ok == true and r[2].exhausted == false, 'the sibling is a plain success')\n\
        assert(r[2].item == 'plain', 'the sibling keeps its item')\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        if item == 'loop' then\n\
          local msgs = messages.new()\n\
          msgs:user('loop forever')\n\
          models.loop(msgs)\n\
          return 'unreachable'\n\
        end\n\
        return 'plain:' .. item\n\
        ```\n"
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an exhausted arm must not fail the fanout");

    assert_eq!(
        out, "## loop\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)|plain:plain",
        "the exhausted slot is the stub and the sibling's result lands"
    );
    assert_eq!(
        gateway.call_count(),
        cap,
        "the looping arm made exactly `cap` round trips before exhausting"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the exhausted arm reports its failure: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        1,
        "the sibling reports its success: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        0,
        "an exhausted arm cancels nothing: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_arms_writing_one_path_terminate_the_run_with_a_determinism_violation() {
    // The arms write unordered: whichever op reaches the backend second
    // meets the first's standing write claim, because claims are never
    // released during a run, so the conflict cannot depend on timing.
    // The violation is fatal to the whole run at the answer boundary -
    // the loser never resumes into Lua, so no author pcall can catch it -
    // and every arm still parked when the fatal answer lands drops
    // unarmed, reporting cancelled rather than failed.
    //
    // The winner's answer may land before or after the loser's: either
    // interleaving satisfies the contract - both arms started, no arm
    // fails on its own, and at most the winner reports a terminal (the
    // loser is stranded mid-chain by the run's own failure, which is the
    // record of how it ended).
    let recorder = Arc::new(Recorder::default());
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('shared.txt', item)\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) =
        scheduler_context_on(&prompt, &store, Arc::clone(&recorder) as Arc<dyn Observer>);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("two arms writing one path must terminate the run");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("shared.txt"), "error was: {detail}");
            assert!(
                detail.contains("conflicts with"),
                "the conflict is named: {detail}"
            );
            assert_eq!(
                detail.matches("ExecId(").count(),
                2,
                "both arms' identities are named: {detail}"
            );
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started before the conflict: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        0,
        "no arm fails on its own; the run ends at the answer boundary: {:?}",
        recorder.events()
    );
    assert!(
        terminal_count(&recorder, TASK_SUCCEEDED) <= 1,
        "at most the winner (whose answer landed first) reports a terminal: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_live_arms_appending_one_path_terminate_with_a_determinism_violation() {
    // The papergate case the WriteScope registry never caught: `append`
    // claims write intent, so two arms appending to one path conflict
    // exactly as two writes do - and under happens-before the conflict is
    // unconditional, because the arms never join each other before the
    // fanout's own rounds deliver them. The conflict is the fatal
    // determinism violation, not a per-arm store error.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('evidence.md', item .. '\\n')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("two arms appending one path must terminate the run");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("evidence.md"), "error was: {detail}");
            assert_eq!(
                detail.matches("ExecId(").count(),
                2,
                "both arms' identities are named: {detail}"
            );
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
}
