//! Whole runs on the calling thread: a nested call with two infers,
//! cancellation while suspended on an infer, the call depth cap, a prose
//! infer through the run's configured client, and a dispatch failure
//! caught by `pcall`.

use super::*;

#[test]
fn nested_call_and_inference_run_end_to_end_on_a_current_thread_runtime() {
    // On the calling thread the nested call and both infers complete in
    // order.
    let gateway = ScriptedChat::new(vec![text("inner answer"), text("outer answer")]);
    let md = "---\nname: gate\ndescription: d\nrunvil: 0\n---\n\n\
        # Gate\n\n\
        ## Outer\n\n\
        ```lua\n\
        local inner = call('## Inner')\n\
        return models.infer('outer saw: ' .. inner)\n\
        ```\n\n\
        ## Inner\n\n\
        ```lua\n\
        return models.infer('inner ask')\n\
        ```\n";
    let trace = scheduler_scenario(md).chat(&gateway).run();

    assert_eq!(trace.text(), "outer answer");
    assert_eq!(
        gateway.call_count(),
        2,
        "the child's infer and the parent's infer each drive one completion"
    );
    let requests = gateway.requests();
    assert_eq!(
        requests[0].messages[0].content(),
        "inner ask",
        "the contained chain's infer runs first: {requests:?}"
    );
    assert_eq!(
        requests[1].messages[0].content(),
        "outer saw: inner answer",
        "the parent resumes with the contained chain's final text: {requests:?}"
    );
}

#[test]
fn cancellation_while_suspended_on_infer_interrupts_the_run() {
    // The 30-second reply is held, and the cancel lands once its round is
    // issued.
    let gateway = ScriptedChat::new(vec![delayed(
        std::time::Duration::from_secs(30),
        text("too late"),
    )]);
    let md = "---\nname: cancel\ndescription: d\nrunvil: 0\n---\n\n\
        # Cancel\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('hang')\n```\n";
    let is_round = |effect: &Effect| matches!(effect, Effect::Chat { .. });
    let trace = scheduler_scenario(md)
        .chat(&gateway)
        .hold(is_round)
        .cancel_when(is_round)
        .run();

    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "cancelling a suspended infer must interrupt the run, got {:?}",
        trace.result()
    );
    assert!(
        matches!(
            trace.effects(),
            [(EffectRecord::Chat { .. }, AnswerRecord::Dropped)]
        ),
        "the cancellation must occur after infer reached the caller: {:?}",
        trace.effects()
    );
}

#[test]
fn call_depth_cap_reads_the_chain_field() {
    // Two sections calling each other ping-pong through nested call
    // chains; the cap must fire from the requesting chain's call-depth
    // field. The typed error then round-trips through every parent's
    // answer envelope without flattening, and classifies as a quota.
    let md = "---\nname: depth\ndescription: d\nrunvil: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\nreturn call('## Beta')\n```\n\n\
        ## Beta\n\n\
        ```lua\nreturn call('## Alpha')\n```\n";
    let trace = scheduler_scenario(md).run();

    let error = trace.error();
    assert!(
        matches!(
            error.inner(),
            Error::CallDepthExceeded {
                cap: 8,
                via: DepthVia::Call
            }
        ),
        "expected the typed call-depth error, got {error:?}"
    );
    assert_eq!(error.kind(), RunErrorKind::Quota, "{error}");
    assert_eq!(error.to_string(), "call recursion exceeded cap of 8");
}

#[test]
fn a_lua_infer_of_prose_uses_the_run_configured_client() {
    // The chain's client slot is seeded from the run's configured client, so
    // a section's explicit `models.infer(prose)` reaches that gateway rather
    // than falling back to an environment client; the returned text becomes
    // the run's result.
    let gateway = ScriptedChat::new(vec![text("prose answer")]);
    let md = "---\nname: prose\ndescription: d\nrunvil: 0\n---\n\n\
        # Prose\n\n\
        ## Only\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let trace = scheduler_scenario(md).chat(&gateway).run();

    assert_eq!(trace.text(), "prose answer");
    assert_eq!(gateway.call_count(), 1, "the infer drives one completion");
    let requests = gateway.requests();
    let content = requests[0].messages[0].content();
    assert!(
        content.contains("Say something."),
        "the prose text reaches the gateway: {requests:?}"
    );
}

#[test]
fn a_dispatch_failure_resumes_through_the_envelope_into_pcall() {
    // A failed dispatch (here an unresolvable call target) is the call's
    // answer resumed through the error envelope, so an author `pcall`
    // catches it; a driver that failed the chain instead would error the
    // run.
    let md = "---\nname: catch\ndescription: d\nrunvil: 0\n---\n\n\
        # Catch\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(call, '## Missing')\n\
        if ok then return 'uncaught' end\n\
        return 'caught: ' .. tostring(err)\n\
        ```\n";
    let trace = scheduler_scenario(md).run();

    let out = trace.text();
    assert!(
        out.starts_with("caught: "),
        "the pcall catches the dispatch failure, got {out:?}"
    );
    assert!(
        out.contains("not found"),
        "the caught error is the target resolution failure, got {out:?}"
    );
}
