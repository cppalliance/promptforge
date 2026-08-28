//! Scheduler-side tests: the decision-gate scenario (nested execute plus
//! inference end-to-end on a current-thread runtime), cancellation while
//! suspended on an infer, and the per-chain execute-depth cap.

use super::*;
use crate::execute::scheduler::Scheduler;
use crate::model::{ModelBinding, ModelId, ModelInvocation};

/// The model set the live H1 pass would leave behind: one `writer` binding
/// as the prompt-wide default. The scheduler's tests bypass H1, so they
/// pre-fill the run's shared set directly.
fn writer_models() -> ModelSet {
    ModelSet {
        bindings: vec![ModelBinding::new(
            "writer",
            "A general model for tests",
            ModelId::from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )],
        default: Some("writer".to_owned()),
    }
}

/// Builds the run context for a scheduler test: the parsed prompt, an empty
/// shared library, and the model set pre-filled.
fn scheduler_context(prompt: &Prompt) -> RunContext {
    let ctx = RunContext::new(
        prompt,
        "",
        &StoreRef::memory(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunConfig::new(EXECUTION),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = writer_models();
    ctx
}

#[tokio::test(flavor = "current_thread")]
async fn nested_execute_and_inference_run_end_to_end_on_a_current_thread_runtime() {
    // THE DECISION GATE: under the legacy bridge this prompt fails with
    // `Error::Internal` on a current-thread runtime; under the scheduler
    // the nested execute and both infers complete on the one thread.
    let gateway =
        ScriptedGateway::start(vec![resp_text("inner answer"), resp_text("outer answer")]).await;
    let md = "---\nname: gate\ndescription: d\npromptforge: 1\n---\n\n\
        # Gate\n\n\
        ## Outer\n\n\
        ```lua\n\
        local inner = execute('## Inner')\n\
        return models.infer('outer saw: ' .. inner)\n\
        ```\n\n\
        ## Inner\n\n\
        ```lua\n\
        return models.infer('inner ask')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
    use crate::cancel::{self, CancelHandle};

    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let md = "---\nname: cancel\ndescription: d\npromptforge: 1\n---\n\n\
        # Cancel\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('hang')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let cancel = CancelHandle::new();
    let canceller = cancel.clone();
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

    let result = cancel::scope(cancel, async {
        Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
            .drive()
            .await
    })
    .await;

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
async fn execute_depth_cap_reads_the_chain_field() {
    // Two sections executing each other ping-pong down the chain stack; the
    // cap must fire from the requesting chain's execute-depth field. The
    // typed error then round-trips through every parent's answer envelope
    // without flattening.
    let md = "---\nname: depth\ndescription: d\npromptforge: 1\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\nreturn execute('## Beta')\n```\n\n\
        ## Beta\n\n\
        ```lua\nreturn execute('## Alpha')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let error = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "execute recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_prose_block_uses_the_run_configured_client_and_binds_the_reply() {
    // The chain's client slot is seeded from the run's configured client, so
    // a prose block before any infer reaches that gateway rather than
    // falling back to an environment client; the prose output binds the
    // reply that becomes the run's result.
    let gateway = ScriptedGateway::start(vec![resp_text("prose answer")]).await;
    let md = "---\nname: prose\ndescription: d\npromptforge: 1\n---\n\n\
        # Prose\n\n\
        ## Only\n\n\
        Say something.\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a prose block runs through the scheduler");

    assert_eq!(out, "prose answer");
    assert_eq!(
        gateway.call_count(),
        1,
        "the prose block drives one completion"
    );
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
async fn a_lua_blocks_reply_read_back_steers_the_chain_result() {
    // A Lua block that sets `reply` without returning falls through; the
    // read-back after the block is what the chain's finish reports. Without
    // the read-back the run would end with the generic completion.
    let md = "---\nname: reply\ndescription: d\npromptforge: 1\n---\n\n\
        # Reply\n\n\
        ## Only\n\n\
        ```lua\nreply = 'from lua'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a reply-setting block runs through the scheduler");

    assert_eq!(out, "from lua");
}

#[tokio::test(flavor = "current_thread")]
async fn a_dispatch_failure_resumes_through_the_envelope_into_pcall() {
    // A failed dispatch (here an unresolvable execute target) is the call's
    // answer resumed through the error envelope, so an author `pcall`
    // catches it exactly as on the legacy callback path; a driver that
    // failed the chain instead would error the run.
    let md = "---\nname: catch\ndescription: d\npromptforge: 1\n---\n\n\
        # Catch\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(execute, '## Missing')\n\
        if ok then return 'uncaught' end\n\
        return 'caught: ' .. tostring(err)\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
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
