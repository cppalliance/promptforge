//! Whole runs on a current-thread runtime: a nested call with two infers,
//! cancellation while suspended on an infer, the call depth cap, a prose
//! infer through the run's configured client, and a dispatch failure
//! caught by `pcall`.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn nested_call_and_inference_run_end_to_end_on_a_current_thread_runtime() {
    // On a current-thread runtime the nested call and both infers complete
    // on the one thread.
    let gateway =
        ScriptedGateway::start(vec![resp_text("inner answer"), resp_text("outer answer")]).await;
    let md = "---\nname: gate\ndescription: d\npromptforge: 0\n---\n\n\
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
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, harness, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the gate scenario runs end to end on one thread");

    assert_eq!(out, "outer answer");
    assert_eq!(
        gateway.call_count(),
        2,
        "the child's infer and the parent's infer each drive one completion"
    );
    let requests = gateway.requests();
    assert_eq!(
        requests[0]["messages"][0]["content"].as_str(),
        Some("inner ask"),
        "the contained chain's infer runs first: {requests:?}"
    );
    assert_eq!(
        requests[1]["messages"][0]["content"].as_str(),
        Some("outer saw: inner answer"),
        "the parent resumes with the contained chain's final text: {requests:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_while_suspended_on_infer_interrupts_the_run() {
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let md = "---\nname: cancel\ndescription: d\npromptforge: 0\n---\n\n\
        # Cancel\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('hang')\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let mut driver = TokioDriver::new(&ctx, harness, Some(gateway_client(gateway.addr())));
    let canceller = driver.cancel_handle();
    let calls = Arc::clone(&gateway.calls);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let result = driver.drive().await;

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling a suspended infer must interrupt the run, got {result:?}"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the cancellation must occur after infer reached the gateway"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn call_depth_cap_reads_the_chain_field() {
    // Two sections calling each other ping-pong through nested call
    // chains; the cap must fire from the requesting chain's call-depth
    // field. The typed error then round-trips through every parent's
    // answer envelope without flattening.
    let md = "---\nname: depth\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\nreturn call('## Beta')\n```\n\n\
        ## Beta\n\n\
        ```lua\nreturn call('## Alpha')\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "call recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_lua_infer_of_prose_uses_the_run_configured_client() {
    // The chain's client slot is seeded from the run's configured client, so
    // a section's explicit `models.infer(prose)` reaches that gateway rather
    // than falling back to an environment client; the returned text becomes
    // the run's result.
    let gateway = ScriptedGateway::start(vec![resp_text("prose answer")]).await;
    let md = "---\nname: prose\ndescription: d\npromptforge: 0\n---\n\n\
        # Prose\n\n\
        ## Only\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, harness, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an explicit infer of the prose runs through the scheduler");

    assert_eq!(out, "prose answer");
    assert_eq!(gateway.call_count(), 1, "the infer drives one completion");
    let requests = gateway.requests();
    let content = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        content.contains("Say something."),
        "the prose text reaches the gateway: {requests:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_dispatch_failure_resumes_through_the_envelope_into_pcall() {
    // A failed dispatch (here an unresolvable call target) is the call's
    // answer resumed through the error envelope, so an author `pcall`
    // catches it; a driver that failed the chain instead would error the
    // run.
    let md = "---\nname: catch\ndescription: d\npromptforge: 0\n---\n\n\
        # Catch\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(call, '## Missing')\n\
        if ok then return 'uncaught' end\n\
        return 'caught: ' .. tostring(err)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("the dispatch failure is catchable");

    assert!(
        out.starts_with("caught: "),
        "the pcall catches the dispatch failure, got {out:?}"
    );
    assert!(
        out.contains("not found"),
        "the caught error is the target resolution failure, got {out:?}"
    );
}
