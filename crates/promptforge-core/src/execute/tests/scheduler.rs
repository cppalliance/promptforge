//! Scheduler-side tests: the decision-gate scenario (nested execute plus
//! inference end-to-end on a current-thread runtime), cancellation while
//! suspended on an infer, the per-chain execute-depth cap, and the walk
//! rules mirrored from the legacy suite (fall-through order, off-walk
//! skips, reply roll-forward, `var` discipline, the run-global id
//! counter), plus the control-transfer rules: jump targets (sibling moves
//! and child descents with the parent resuming after the jumper), the
//! scalar return's chain scoping, and the section-boundary observations.

use std::num::NonZeroUsize;

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
    scheduler_context_on(prompt, &StoreRef::memory(), Arc::new(NullObserver))
}

/// Builds the run context on the given store and observer, so a walk test
/// can inspect the store's contents and the observation stream afterward.
fn scheduler_context_on(
    prompt: &Prompt,
    store: &StoreRef,
    observer: Arc<dyn Observer>,
) -> RunContext {
    let ctx = RunContext::new(
        prompt,
        "",
        store,
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunConfig::new(EXECUTION).observer(observer),
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

// --- Walk translation: the core rules mirrored from the legacy suite ---
// Each test names the legacy case it mirrors. The legacy cases keep
// exercising the legacy engine untouched; these prove the scheduler.

#[tokio::test(flavor = "current_thread")]
async fn sections_run_in_fall_through_order() {
    // Mirror of the legacy `falls_through_to_next_section`, strengthened
    // with an order log: a section without a return falls through to the
    // next section in document order.
    let store = StoreRef::memory();
    let md = "---\nname: walk\ndescription: d\npromptforge: 1\n---\n\n\
        # Walk\n\n\
        ## First\n\n\
        ```lua\nstore.append('order.txt', 'First\\n')\n```\n\n\
        ## Second\n\n\
        ```lua\nstore.append('order.txt', 'Second\\n')\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the walk falls through in document order");

    assert_eq!(out, "First\nSecond\n");
}

#[tokio::test(flavor = "current_thread")]
async fn generic_result_when_nothing_produced() {
    // Mirror of the legacy `generic_result_when_nothing_produced`: a walk
    // that exhausts its slice with no reply yields the shared generic
    // completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Generic\n\n\
        ## Only\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the empty walk completes");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_increments_per_section() {
    // Mirror of the legacy `sys_id_increments_per_section`: every section
    // entry takes the next run-global id.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Ids\n\n\
        ## First\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Second\n\n\
        ```lua\nreturn tostring(sys.id)\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("each section entry takes the next id");

    assert_eq!(out, "2");
}

#[tokio::test(flavor = "current_thread")]
async fn off_walk_section_is_never_visited_by_the_walk() {
    // Mirror of the legacy case of the same name: an off-walk section is
    // skipped without a frame - no execution, no observation - while
    // `sys.section_count` still counts it.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Skip\n\n\
        ## Hidden\n\n\
        ---\n\n\
        ```lua\nerror('hidden must not run')\n```\n\n\
        ## Main\n\n\
        ```lua\n\
        assert(sys.section_count == 2)\n\
        return 'main-ran'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the walk must skip the off-walk section");

    assert_eq!(out, "main-ran");
    let records = recorder.records();
    assert!(
        records.iter().all(|(_, section, _)| section != "Hidden"),
        "an off-walk section must produce no observations: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn off_walk_sections_run_only_when_addressed() {
    // Mirror of the legacy case of the same name: A executes B and C (both
    // off-walk), D unmarked - the walk visits A then D, and the off-walk
    // sections run when addressed.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Addressed\n\n\
        ## A\n\n\
        ```lua\n\
        local rb = execute('## B')\n\
        local rc = execute('## C')\n\
        store.write('order.txt', rb .. ',' .. rc)\n\
        ```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nreturn 'b-ran'\n```\n\n\
        ## C\n\n\
        ---\n\n\
        ```lua\nreturn 'c-ran'\n```\n\n\
        ## D\n\n\
        ```lua\nreturn 'd-ran:' .. store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("off-walk sections must run when addressed");

    // A walked B or C would end the run early with its own scalar return.
    assert_eq!(out, "d-ran:b-ran,c-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn a_contained_chain_skips_off_walk_sections_in_fall_through() {
    // Mirror of the legacy case of the same name: a contained chain skips
    // off-walk sections in fall-through like any walk - only addressing
    // runs a marked section.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Contained\n\n\
        ## A\n\n\
        ```lua\n\
        local r = execute('## Sub')\n\
        return r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\nstore.append('order.txt', 'Sub\\n')\n```\n\n\
        ## Hidden\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'Hidden\\n')\n```\n\n\
        ## Tail\n\n\
        ```lua\n\
        store.append('order.txt', 'Tail\\n')\n\
        return 'tail-reply'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the chain must skip the off-walk section in fall-through");

    assert_eq!(out, "tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nTail\n");
}

#[tokio::test(flavor = "current_thread")]
async fn execute_chain_over_off_walk_siblings_returns_to_the_caller() {
    // Mirror of the legacy case of the same name: A executes the off-walk
    // S1, which runs because it is addressed; the chain falls through to
    // S2, and S2's reply returns to A. The main walk ends at B and never
    // runs S1 or S2.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Siblings\n\n\
        ## A\n\n\
        ```lua\n\
        local r = execute('## S1')\n\
        store.append('order.txt', 'A:' .. r .. '\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## S1\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'S1\\n')\n```\n\n\
        ## S2\n\n\
        ```lua\n\
        store.append('order.txt', 'S2\\n')\n\
        return 's2-reply'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the chain must run the addressed off-walk target and fall through");

    assert_eq!(out, "S1\nS2\nA:s2-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn reply_carries_forward_to_next_section() {
    // Mirror of the legacy `reply_carries_forward_to_next_section_prologue`:
    // one section's prose-produced reply seeds the next section's VM.
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Reply\n\n\
        ## First\n\n\
        Ask the model.\n\n\
        ## Second\n\n\
        ```lua\nreturn reply\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the reply must carry forward across the section boundary");

    assert_eq!(out, "hello from the mock");
}

#[tokio::test(flavor = "current_thread")]
async fn reply_assignment_in_lua_carries_to_next_section() {
    // Mirror of the legacy case of the same name: the global IS the reply,
    // so an author's `reply = "custom"` carries to the next section exactly
    // like a prose-produced reply.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Assign\n\n\
        ## Source\n\n\
        ```lua\nreply = 'custom'\n```\n\n\
        ## Next\n\n\
        ```lua\n\
        assert(reply == 'custom', 'a Lua reply assignment must carry to the next section')\n\
        return reply\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a Lua reply assignment must carry to the next section");

    assert_eq!(out, "custom");
}

#[tokio::test(flavor = "current_thread")]
async fn reply_nil_before_fall_through_clears_reply_for_next_section() {
    // Mirror of the legacy case of the same name: `reply = nil` as a
    // section's last word clears the reply the next section on the walk
    // sees.
    let gateway = ScriptedGateway::start(vec![resp_text("model-said-this")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Clear\n\n\
        ## Source\n\n\
        Ask something.\n\n\
        ```lua\nreply = nil\n```\n\n\
        ## Next\n\n\
        ```lua\n\
        assert(reply == nil, 'reply = nil at fall-through must clear what the next section sees')\n\
        return 'cleared'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("reply = nil at fall-through must clear the reply the next section sees");

    assert_eq!(out, "cleared");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_sections_in_fall_through() {
    // Mirror of the fall-through half of the legacy
    // `var_persists_across_sections_fallthrough_and_jump` (its jump half
    // lands with the jump translation): one section's `var` writes reach
    // the next across fall-through.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Var\n\n\
        ## A\n\n\
        ```lua\nvar.from_a = 'a'\n```\n\n\
        ## B\n\n\
        ```lua\n\
        assert(var.from_a == 'a', 'fall-through keeps the walk var')\n\
        var.from_b = 'b'\n\
        ```\n\n\
        ## C\n\n\
        ```lua\nreturn var.from_a .. var.from_b\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("var must persist across the walk");

    assert_eq!(out, "ab");
}

#[tokio::test(flavor = "current_thread")]
async fn execute_clones_var_in_and_discards_child_writes() {
    // Mirror of the legacy case of the same name: `execute` clones the
    // caller's `var` in; the contained chain reads the clone, and its
    // writes are discarded when the chain ends.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Clone\n\n\
        ## Main\n\n\
        ```lua\n\
        var.shared = 'caller'\n\
        local r = execute('## Sub')\n\
        assert(r == 'sub saw caller', 'the child reads the cloned var')\n\
        assert(var.child_write == nil, 'child writes must not reach the caller')\n\
        return 'ok'\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        var.child_write = 'sub'\n\
        return 'sub saw ' .. var.shared\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("execute must clone var in and discard child writes");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn an_execute_chain_continues_the_global_sys_id_sequence() {
    // Mirror of the legacy case of the same name: the contained chain's
    // entries take the next run-global ids, and the outer walk resumes the
    // same sequence when the chain ends.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Sequence\n\n\
        ## Main\n\n\
        ```lua\n\
        assert(sys.id == 1, 'the first walked section takes id 1')\n\
        local r = execute('## Sub')\n\
        store.append('order.txt', r .. '\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        assert(sys.id == 4, 'the outer walk resumes the global sequence')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        assert(sys.id == 2, 'the contained chain continues the global sequence')\n\
        ```\n\n\
        ## Tail\n\n\
        ```lua\n\
        assert(sys.id == 3, 'the chain fall-through takes the next global id')\n\
        return 'tail-reply'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("an execute chain must continue the global sys.id sequence");

    assert_eq!(out, "tail-reply\n");
}

#[tokio::test(flavor = "current_thread")]
async fn entering_the_same_section_twice_takes_two_ids() {
    // Mirror of the legacy case of the same name: entering the same
    // section twice hands out two run-global `sys.id` values.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Twice\n\n\
        ## Main\n\n\
        ```lua\n\
        local a = execute('## Sub')\n\
        local b = execute('## Sub')\n\
        return a .. ',' .. b\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\nreturn tostring(sys.id)\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("re-entering a section must take a fresh id");

    assert_eq!(out, "2,3");
}

#[tokio::test(flavor = "current_thread")]
async fn fall_through_fires_section_finished_before_the_next_section_starts() {
    // The boundary half of the legacy
    // `a_two_section_run_reports_the_exact_observation_sequence`: each
    // entered section's armed frame drop fires SECTION_FINISHED at the
    // fall-through, before the next section's SECTION_STARTED.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Boundaries\n\n\
        ## One\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Two\n\n\
        ```lua\nreturn 'two-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the walk completes both sections");

    assert_eq!(out, "two-ran");
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
            ("One".to_owned(), started.clone()),
            ("One".to_owned(), finished.clone()),
            ("Two".to_owned(), started.clone()),
            ("Two".to_owned(), finished.clone()),
        ]
    );
}

// --- Walk translation: jumps, returns, and observation boundaries ---
// Each test names the legacy case it mirrors. The legacy cases keep
// exercising the legacy engine untouched; these prove the scheduler.

#[tokio::test(flavor = "current_thread")]
async fn jump_target_sees_no_prior_reply_and_transfer_skips_remaining_blocks() {
    // Mirror of the legacy case of the same name: the jump transfers
    // control, the jumper's remaining blocks never run, and the target
    // sees no prior reply because no prose ran before the jump.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
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
        assert(reply == nil, 'no prior reply because no prose ran before the jump')\n\
        return 'helped:' .. store.read('seen.txt')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("jump must transfer control");

    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_preserves_reply_from_prior_section() {
    // Mirror of the legacy case of the same name: a jump carries the prior
    // section's reply across the transfer.
    let gateway = ScriptedGateway::start(vec![resp_text("model-said-this")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Carry\n\n\
        ## Source\n\n\
        Ask something.\n\n\
        ```lua\njump('## Target')\n```\n\n\
        ## Target\n\n\
        ```lua\n\
        assert(reply ~= nil, 'jump must preserve the prior reply')\n\
        return reply\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("jump must preserve the reply from the prior section");

    assert_eq!(out, "model-said-this");
}

#[tokio::test(flavor = "current_thread")]
async fn reply_nil_before_jump_clears_reply_for_target() {
    // Mirror of the legacy case of the same name: `reply = nil` before a
    // jump clears the reply the jump target sees - the walk reads the Lua
    // `reply` global back at the transfer.
    let gateway = ScriptedGateway::start(vec![resp_text("model-said-this")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Clear\n\n\
        ## Source\n\n\
        Ask something.\n\n\
        ```lua\n\
        reply = nil\n\
        jump('## Target')\n\
        ```\n\n\
        ## Target\n\n\
        ```lua\n\
        assert(reply == nil, 'reply = nil before the jump must clear what the target sees')\n\
        return 'cleared'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("reply = nil before a jump must clear the reply the target sees");

    assert_eq!(out, "cleared");
}

#[tokio::test(flavor = "current_thread")]
async fn section_cannot_jump_to_itself() {
    // Mirror of the legacy case of the same name: the caller is outside its
    // own visible set, so naming its own heading to `jump` resolves as
    // not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Self\n\n\
        ## Self\n\n\
        ```lua\njump('## Self')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let error = Scheduler::new(&ctx, None)
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
    // Mirror of the legacy case of the same name: a jump addresses an
    // off-walk section directly, so it runs.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Addressed\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nreturn 'b-ran'\n```\n\n\
        ## C\n\n\
        ```lua\nreturn 'c-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump to an off-walk section must run it");

    assert_eq!(out, "b-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn fall_through_after_a_jumped_off_walk_section_skips_again() {
    // Mirror of the legacy case of the same name: a jump runs its off-walk
    // target, but the fall-through that follows is an ordinary walk step -
    // the next off-walk sibling is skipped again.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Once\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nlocal b = 1\n```\n\n\
        ## C\n\n\
        ---\n\n\
        ```lua\nreturn 'c-ran'\n```\n\n\
        ## D\n\n\
        ```lua\nreturn 'd-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("fall-through after an addressed off-walk section must resume skipping");

    assert_eq!(out, "d-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_a_jump() {
    // The jump half of the legacy
    // `var_persists_across_sections_fallthrough_and_jump` (its H1-seed half
    // has no scheduler counterpart - the scheduler's drive starts at the
    // walk): the jumper's `var` writes cross the transfer, and the target's
    // writes roll forward into the fall-through that follows.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
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
        assert(var.from_a == 'a', 'the jump carries the jumper writes')\n\
        var.from_c = 'c'\n\
        ```\n\n\
        ## D\n\n\
        ```lua\n\
        assert(var.from_c == 'c', 'fall-through after the jumped target keeps var')\n\
        return var.from_a .. var.from_c\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
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
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Boundaries\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let out = Scheduler::new(&ctx, None)
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
    // Mirror of the legacy case of the same name: a section that errors
    // mid-walk emits SECTION_STARTED and never SECTION_FINISHED - the
    // frame's drop stays unarmed on the error path.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fail\n\n\
        ## Only\n\n\
        ```lua\nerror('expected failure')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let result = Scheduler::new(&ctx, None).drive().await;

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
async fn jump_to_a_child_starts_the_child_level_walk() {
    // Mirror of the legacy case of the same name: a jump to an H3 child
    // starts a child-level walk at the target, which falls through to the
    // target's following siblings; when the level exhausts, the parent walk
    // resumes after the jumper.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Descend\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump to a child must start the child-level walk");

    assert_eq!(out, "A\nX\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn child_walk_reply_thread_follows_the_detour() {
    // Mirror of the legacy case of the same name: the jumper's reply
    // reaches the sub-walk's first section, each section of the sub-walk
    // rolls it forward, and the sub-walk's last reply resumes the parent
    // chain.
    let gateway = ScriptedGateway::start(vec![
        resp_text("reply-a"),
        resp_text("reply-x"),
        resp_text("reply-y"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Detour\n\n\
        ## A\n\n\
        Ask A.\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\nassert(reply == 'reply-a', 'the jumper reply reaches the first child')\n```\n\n\
        Ask X.\n\n\
        ### Y\n\n\
        ```lua\nassert(reply == 'reply-x', 'the child walk rolls the reply forward')\n```\n\n\
        Ask Y.\n\n\
        ## B\n\n\
        ```lua\n\
        assert(reply == 'reply-y', 'the sub-walk last reply resumes the parent chain')\n\
        return reply\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the reply thread must follow the detour");

    assert_eq!(out, "reply-y");
}

#[tokio::test(flavor = "current_thread")]
async fn child_walk_recurses_to_h4() {
    // Mirror of the legacy case of the same name: the child-level rule
    // recurses - a jump from an H3 child to an H4 grandchild starts an
    // H4-level walk, and each level's exhaustion resumes its parent after
    // the jumper.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Recurse\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('order.txt', 'X\\n')\n\
        jump('#### P')\n\
        ```\n\n\
        #### P\n\n\
        ```lua\nstore.append('order.txt', 'P\\n')\n```\n\n\
        #### Q\n\n\
        ```lua\nstore.append('order.txt', 'Q\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the child-level rule must recurse to H4");

    assert_eq!(out, "A\nX\nP\nQ\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn off_walk_child_is_skipped_by_the_child_walk() {
    // Mirror of the legacy case of the same name: the off-walk flag applies
    // at every walked level - an off-walk H3 is skipped by the child-level
    // walk's fall-through.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Skip\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Off\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'Off\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the child walk must skip the off-walk child");

    assert_eq!(out, "X\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_an_off_walk_child_runs_it() {
    // Mirror of the legacy case of the same name: an off-walk child stays
    // addressable - a jump to it runs it, and the fall-through that follows
    // skips nothing addressed.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # OffChild\n\n\
        ## A\n\n\
        ```lua\njump('### Off')\n```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Off\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'Off\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump to an off-walk child must run it");

    assert_eq!(out, "Off\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_addresses_its_own_siblings_and_children() {
    // Mirror of the legacy case of the same name: a running child's visible
    // set is its own siblings plus its own children - it can execute a
    // child and jump to a sibling.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Visible\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        local r = execute('#### Grand')\n\
        store.append('order.txt', 'X:' .. r .. '\\n')\n\
        jump('### Y')\n\
        ```\n\n\
        #### Grand\n\n\
        ```lua\nreturn 'grand-ran'\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a running child must address its own siblings and children");

    assert_eq!(out, "X:grand-ran\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_cannot_address_a_top_level_section() {
    // Mirror of the legacy case of the same name: a running child cannot
    // address a top-level section - the parent level is not in its visible
    // set, so the jump resolves as not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Escape\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let error = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect_err("a child jumping to a top-level section must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a top-level section is not in a child's visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_a_niece_errors() {
    // Mirror of the legacy case of the same name: a sibling's child (a
    // niece or nephew) is not in the visible set, so the jump resolves as
    // not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Niece\n\n\
        ## A\n\n\
        ```lua\njump('### Niece')\n```\n\n\
        ## B\n\n\
        ### Niece\n\n\
        ```lua\nreturn 'niece-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let error = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect_err("a jump to a niece must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_counts_sections_entered_run_wide() {
    // Mirror of the legacy case of the same name: `sys.id` counts the
    // sections the walk has entered run-wide - the detour into a child
    // level continues the count rather than restarting it.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Ids\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        return store.read('ids.txt')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("sys.id must count sections entered run-wide");

    assert_eq!(out, "1\n2\n3\n4\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_child_walk_ends_the_whole_chain() {
    // The rule-5 clause the legacy cases imply but none isolates: a scalar
    // return inside a jump-started child-level walk ends the whole chain,
    // not just the child level - the parent walk never resumes.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Return\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\nreturn 'x-value'\n```\n\n\
        ## B\n\n\
        ```lua\nerror('the return must end the chain before the parent resumes')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a return in the child walk ends the whole chain");

    assert_eq!(out, "x-value");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_execute_is_contained_in_the_chain() {
    // Mirror of the legacy case of the same name: a jump inside `execute()`
    // is contained by the chain - followed, not rejected. The chain's index
    // moves to the target, the sections between the jumper and the target
    // do not run, and the target's reply returns to the caller.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Contained\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = execute('## Sub')\n\
        return 'main:' .. r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Skipped\n\n\
        ```lua\nerror('the chain jump must move past me')\n```\n\n\
        ## Peer\n\n\
        ```lua\nreturn 'peer-ran'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump inside execute must be followed within the chain");

    assert_eq!(out, "main:peer-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_an_execute_chain_moves_within_the_chain() {
    // Mirror of the legacy case of the same name: a jump inside an
    // `execute()` chain to a sibling moves within the contained chain - the
    // walk continues from the jump target under the normal rules, and the
    // chain's final reply is the call's return value.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Move\n\n\
        ## A\n\n\
        ```lua\n\
        local r = execute('## Sub')\n\
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
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump inside the chain must move within the chain");

    assert_eq!(out, "A:tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Peer\nTail\n");
}

#[tokio::test(flavor = "current_thread")]
async fn execute_chain_jumps_to_a_child_and_returns_the_chain_reply() {
    // Mirror of the legacy case of the same name (the canonical contained
    // chain): A executes Sub; Sub jumps to its child S1, starting a
    // child-level walk that falls through to S2; when S2 finishes, the
    // chain's final reply returns to A and the outer walk continues at B,
    // never having moved.
    let gateway = ScriptedGateway::start(vec![resp_text("reply-s1"), resp_text("reply-s2")]).await;
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Chain\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A1\\n')\n\
        local r = execute('## Sub')\n\
        assert(r == 'reply-s2', 'the chain final reply returns to A')\n\
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
        Ask S1.\n\n\
        ```lua\n\
        assert(reply == 'reply-s1', 'the chain rolls the reply forward')\n\
        store.append('order.txt', 'S1\\n')\n\
        ```\n\n\
        ### S2\n\n\
        ```lua\nassert(reply == 'reply-s1', 'fall-through inside the chain carries the reply')\n```\n\n\
        Ask S2.\n\n\
        ```lua\nstore.append('order.txt', 'S2\\n')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the execute chain must jump, fall through, and return its reply");

    assert_eq!(out, "A1\nSub\nS1\nS2\nA2\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn the_outer_walk_never_moves_during_a_contained_chain() {
    // Mirror of the legacy case of the same name: the outer walk never
    // moves while a contained chain runs - wherever the chain ends, the
    // outer walk resumes at the section after the caller.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Outer\n\n\
        ## A\n\n\
        ```lua\n\
        execute('## Sub')\n\
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
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the outer walk must resume at the section after the caller");

    assert_eq!(out, "Peer\nA-done\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    // Mirror of the legacy case of the same name: a return inside a
    // contained chain ends the chain, not the run - the returned value is
    // the call's return, the chain's remaining sections do not run, and
    // the outer walk continues.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Scoped\n\n\
        ## A\n\n\
        ```lua\n\
        local r = execute('## Sub')\n\
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
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a return must end the chain, not the run");

    assert_eq!(out, "A:sub-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn execute_to_a_child_starts_a_contained_chain() {
    // Mirror of the legacy case of the same name: `execute` to a child
    // starts a contained chain at the target - the chain falls through to
    // the target's following siblings under the same rules as any walk, and
    // the chain's final reply is the call's return value.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # ChildExecute\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = execute('### Sub')\n\
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
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("execute to a child must start a contained chain");

    assert_eq!(out, "got:after-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_descent_does_not_consume_execute_depth() {
    // The depth-cap interaction, identical to the legacy engine: a jump
    // descent is not an execute, so the child level shares the chain's
    // execute-depth field. X and Y ping-pong executes from inside a
    // jump-started child walk; each entry appends once. The cap trips when
    // the ninth nested execute would run (depth 9 > 8), after exactly nine
    // section entries - a descent that wrongly consumed depth would trip
    // the cap one entry earlier.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Depth\n\n\
        ## Main\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('depth.txt', 'x\\n')\n\
        return execute('### Y')\n\
        ```\n\n\
        ### Y\n\n\
        ```lua\n\
        store.append('depth.txt', 'y\\n')\n\
        return execute('### X')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let error = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "execute recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
    assert_eq!(
        store.read("depth.txt").expect("depth log"),
        "x\ny\nx\ny\nx\ny\nx\ny\nx\n",
        "the descent shares the chain's execute depth: nine entries, then the cap"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn walk_never_descends_into_children() {
    // Mirror of the legacy case of the same name: the walk never descends -
    // a section's children do not run unless addressed. This is the
    // negative half of the child-descent rule: a fall-through that
    // descended would run the child and trip its error.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # NoDescent\n\n\
        ## A\n\n\
        ```lua\nstore.append('order.txt', 'A\\n')\n```\n\n\
        ### Child\n\n\
        ```lua\nerror('a child must not run by fall-through')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the walk must never descend into children");

    assert_eq!(out, "A\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_jump_resolution_still_finishes_the_jumper() {
    // The error half of the jump observation boundary: the jumper's frame
    // closes as completed before the heading resolves (the legacy walk
    // resolves after the jumper's teardown), so SECTION_FINISHED fires for
    // the jumper even when the target does not resolve.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Unresolved\n\n\
        ## A\n\n\
        ```lua\njump('## Missing')\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let result = Scheduler::new(&ctx, None).drive().await;

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

// --- The live H1 pass on the scheduler ---

/// Builds the run context for a scheduler live-H1 test: the shared model
/// set starts empty - the live H1 pass under test records its own
/// bindings, exactly as the legacy run's H1 hand-off leaves them.
fn h1_context(prompt: &Prompt) -> RunContext {
    h1_context_on(prompt, &StoreRef::memory(), Arc::new(NullObserver))
}

/// Builds the H1 run context on the given store and observer, so a pass
/// test can inspect the store's contents and the observation stream
/// afterward.
fn h1_context_on(prompt: &Prompt, store: &StoreRef, observer: Arc<dyn Observer>) -> RunContext {
    RunContext::new(
        prompt,
        "",
        store,
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunConfig::new(EXECUTION).observer(observer),
    )
}

/// The live H1 resolution inputs for a scheduler test, bundled so the
/// borrows outlive the drive.
struct H1Resolution {
    picker: ToolPicker,
    models: ModelCatalog,
    tools: ToolCatalog,
}

impl H1Resolution {
    /// An empty picker and tool catalog with the test model catalog: H1
    /// model binds resolve, tool binds report absent.
    fn models_only() -> Self {
        Self {
            picker: ToolPicker::build(Catalog::default(), PickerConfig::default())
                .expect("empty tool picker must build"),
            models: test_model_catalog(),
            tools: ToolCatalog::default(),
        }
    }

    /// Everything empty: model binds report absent.
    fn empty() -> Self {
        Self {
            picker: ToolPicker::build(Catalog::default(), PickerConfig::default())
                .expect("empty tool picker must build"),
            models: ModelCatalog::empty(),
            tools: ToolCatalog::default(),
        }
    }

    fn context(&self) -> ResolutionContext<'_> {
        ResolutionContext::new(&self.picker, &self.models, &self.tools)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_infer_runs_once() {
    // Mirror of the legacy case of the same name: the H1 pass binds the
    // default model, a handle's `infer` yields through the shim, and the
    // H1 `var` hand-off seeds the walk.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
    let md = "---\nname: live-h1\ndescription: d\npromptforge: 1\n---\n\n\
        # Live H1\n\n\
        ```lua\n\
        local writer = models.default('writer', 'A general model for tests')\n\
        var.answer = writer:infer('answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::models_only();
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("live H1 path must run on the scheduler");

    assert_eq!(out, "h1 answer");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_models_infer_resolves_the_default_model_without_touching_sys() {
    // Mirror of the legacy case of the same name: the live H1
    // `models.infer` (no handle) resolves the current model from the
    // bindings-so-far and runs the one infer shape - a single tool-free
    // round on a fresh conversation that leaves `sys` untouched.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
    let md = "---\nname: live-h1-models-infer\ndescription: d\npromptforge: 1\n---\n\n\
        # Live H1 Models Infer\n\n\
        ```lua\n\
        models.default('writer', 'A general model for tests')\n\
        var.answer = models.infer('answer once')\n\
        var.sys_untouched = not pcall(function() return sys.reply_finish_reason end)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer .. ':' .. tostring(var.sys_untouched)\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::models_only();
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("live H1 models.infer must run on the scheduler");

    assert_eq!(out, "h1 answer:true");
    assert_eq!(gateway.call_count(), 1);
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(
        body["model"], "claude-sonnet-4-6",
        "models.infer must use the section's current model"
    );
    assert!(
        body.get("tools").is_none(),
        "models.infer advertises no tools: {body}"
    );
    assert_eq!(
        body["messages"].as_array().expect("messages array").len(),
        1,
        "models.infer runs on a fresh context: {body}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_chunk_keeps_sys_id_zero_and_the_first_walked_section_takes_one() {
    // Mirror of the legacy case of the same name: the H1 pass holds id 0
    // off the run-global counter, so the first walked section takes id 1.
    let md = "---\nname: live-h1-sys-id\ndescription: d\npromptforge: 1\n---\n\n\
        # Live H1 Sys Id\n\n\
        ```lua\n\
        assert(sys.id == 0, 'the live H1 chunk keeps sys.id 0')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\n\
        assert(sys.id == 1, 'the first walked section takes sys.id 1')\n\
        return 'ok'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the H1 chunk keeps id 0 and the first walked section takes id 1");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn caught_h1_callback_error_stops_before_a_later_block() {
    // Mirror of the legacy case of the same name: a pcall'd resolver
    // failure is caught by the chunk but recorded by the callback, and the
    // recorded typed error fails the run before the next H1 block runs.
    let md = "---\nname: callback-drain\ndescription: d\npromptforge: 1\n---\n\n\
        # Callback Drain\n\n\
        ```lua\n\
        local ok = pcall(models.bind, 'missing', 'unavailable model')\n\
        assert(not ok)\n\
        ```\n\n\
        ```lua\nstore.write('later.txt', 'ran')\n```\n\n\
        ## Result\n\n\
        ```lua\nreturn 'unexpected'\n```\n";
    let prompt = parse(md);
    let store = StoreRef::memory();
    let ctx = h1_context_on(&prompt, &store, Arc::new(NullObserver));
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("a caught resolver callback error must fail its own block");

    assert!(
        matches!(error, Error::ModelAbsent { .. }),
        "the current block's typed callback error must surface: {error}"
    );
    assert!(
        store.read("later.txt").is_err(),
        "the later H1 block must not run after the callback error"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_caught_h1_callback_error_reports_the_chunk_succeeded() {
    // The observation boundary of the callback-error rule: the chunk
    // caught the resolver's Lua error itself and ran to completion, so it
    // reports LUA_CHUNK_SUCCEEDED; the recorded typed error fails the run
    // only afterward - the legacy `run_live_h1_block` mapping, where the
    // callback check follows the chunk's own boundary.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: callback-drain\ndescription: d\npromptforge: 1\n---\n\n\
        # Callback Drain\n\n\
        ```lua\n\
        local ok = pcall(models.bind, 'missing', 'unavailable model')\n\
        assert(not ok)\n\
        ```\n";
    let prompt = parse(md);
    let ctx = h1_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("the recorded callback error must fail the run");

    assert!(
        matches!(error, Error::ModelAbsent { .. }),
        "the typed callback error must surface: {error}"
    );
    let observed = recorder.events();
    let title = "Callback Drain".to_string();
    assert!(
        observed.contains(&(title.clone(), detail::LUA_CHUNK_SUCCEEDED.to_string())),
        "the chunk that caught the error reports succeeded: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(section, event)| section == &title
                && event == &detail::LUA_CHUNK_FAILED.to_string()),
        "the chunk must not report failed: {observed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_h1_scalar_return_still_reads_var_back() {
    // The read-back half of the H1 return rule: the legacy pass reads the
    // final `var` back on every exit, so a reassigned `var` global fails
    // the run even when the block's scalar return would short-circuit it.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Reassigned Var\n\n\
        ```lua\n\
        var = 5\n\
        return 'early'\n\
        ```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("a reassigned `var` global must fail the run");

    assert!(
        error.to_string().contains("global was reassigned"),
        "the read-back failure must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_is_a_clear_error_on_the_h1() {
    // Mirror of the legacy case of the same name: the H1 VM's control
    // globals are stubs - H1 runs before sections exist, so calling one
    // fails the run with a message naming the cause. On the scheduler the
    // stub must survive the shim base install.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n\
        ```lua\nexecute('## Nope')\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("execute from the H1 must fail with the stub error");

    assert!(
        error.to_string().contains("only available in sections"),
        "the stub error must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_is_a_clear_error_on_the_h1() {
    // Mirror of the legacy case of the same name: `jump` from the H1 hits
    // the stub - the run fails with the clear message, never a recorded
    // jump.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n\
        ```lua\njump('## Nope')\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("jump from the H1 must fail with the stub error");

    assert!(
        error.to_string().contains("only available in sections"),
        "the stub error must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_is_a_clear_error_on_the_h1() {
    // Mirror of the legacy case of the same name: `fanout` from the H1
    // hits the same stub.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n\
        ```lua\nfanout('## Nope', {'a'})\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("fanout from the H1 must fail with the stub error");

    assert!(
        error.to_string().contains("only available in sections"),
        "the stub error must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn list_from_section_is_a_clear_error_on_the_h1() {
    // Mirror of the legacy case of the same name: `list_from_section` from
    // the H1 hits the same stub.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n\
        ```lua\nlist_from_section('## Nope')\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("list_from_section from the H1 must fail with the stub error");

    assert!(
        error.to_string().contains("only available in sections"),
        "the stub error must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_return() {
    // Mirror of the legacy case of the same name: an H1-only prompt's
    // scalar return is the run's result.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Title\n\n\
        ```lua\nreturn \"hello\"\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the H1-only return runs");

    assert_eq!(out, "hello");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_no_return() {
    // Mirror of the legacy case of the same name: an H1-only prompt that
    // produces nothing ends in the shared generic completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Title\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the H1-only fall-through runs");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_scalar_return_short_circuits_the_walk() {
    // The short-circuit half of the H1 return rule: a scalar return from
    // the live H1 pass ends the whole run, so no section ever runs - the
    // walk's erroring section is the tripwire.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Short Circuit\n\n\
        ```lua\nreturn 'early'\n```\n\n\
        ## Never\n\n\
        ```lua\nerror('the walk must not start after an H1 return')\n```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the H1 return short-circuits the run");

    assert_eq!(out, "early");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_prose_reply_is_the_run_result() {
    // The reply half of the empty-sections rule: an H1-only prompt whose
    // prose produces a reply ends the run with that reply, not the generic
    // completion.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Only Prose\n\n\
        ```lua\n\
        models.default('writer', 'A general model for tests')\n\
        ```\n\n\
        say something\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::models_only();
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the H1 prose reply ends the run");

    assert_eq!(out, "h1 reply");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn h1_and_h2_prose_both_run_through_the_shared_block_loop() {
    // Mirror of the legacy case of the same name: the live H1 prose and
    // the H2 section prose each reach the gateway exactly once, in source
    // order.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply"), resp_text("h2 reply")]).await;
    let md = "---\nname: shared-loop\ndescription: d\npromptforge: 1\n---\n\n\
        # Shared Loop\n\n\
        ```lua\n\
        models.default('writer', 'A general model for tests')\n\
        ```\n\n\
        h1 prose turn\n\n\
        ## Section Two\n\n\
        h2 prose turn\n\n\
        ```lua\n\
        return reply\n\
        ```\n";
    let prompt = parse(md);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::models_only();
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("H1 prose and H2 prose both run through the shared block loop");

    assert_eq!(out, "h2 reply");
    assert_eq!(
        gateway.call_count(),
        2,
        "the H1 prose and the H2 prose each drive exactly one completion"
    );
    let requests = gateway.requests();
    let first_prose = requests[0]["messages"][0]["content"]
        .as_str()
        .expect("the first request carries a user message");
    let second_prose = requests[1]["messages"][0]["content"]
        .as_str()
        .expect("the second request carries a user message");
    assert!(
        first_prose.contains("h1 prose turn"),
        "the first completion is the H1 prose: {first_prose}"
    );
    assert!(
        second_prose.contains("h2 prose turn"),
        "the second completion is the H2 prose: {second_prose}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_substitutes_and_skips_empty_prose_before_requiring_a_model() {
    // Mirror of the legacy case of the same name: H1 prose that
    // substitutes to empty never reaches inference, so it never requires a
    // model; non-empty substituted prose still does.
    let empty = "---\nname: empty-h1\ndescription: d\npromptforge: 1\n---\n\n\
        # Empty H1\n\n\
        ```lua\nvar.omit = ''\n```\n\n\
        {{ var.omit }}\n\n\
        ## Result\n\n\
        ```lua\nreturn 'ok'\n```\n";
    let prompt = parse(empty);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("H1 prose that substitutes to empty must not require a model");
    assert_eq!(out, "ok");

    let nonempty = empty.replace("var.omit = ''", "var.omit = 'ask'");
    let prompt = parse(&nonempty);
    let ctx = h1_context(&prompt);
    let resolution = H1Resolution::empty();
    let error = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect_err("non-empty substituted H1 prose must still require a model");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_prose_preserves_non_final_and_final_semantics_and_captures_var() {
    // Mirror of the legacy case of the same name: the live H1 prose runs
    // the always-scope tool loop, a non-final prose block leaves `reply`
    // unset for the following Lua block, the final prose's reply is
    // visible, and `var` writes accumulate across the pass into the walk.
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let echo = Arc::new(EchoTool);
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        echo.description(),
        echo.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize tool capability");
    let md = format!(
        "---\nname: live-h1-prose\ndescription: d\npromptforge: 1\n---\n\n\
         # Live H1 Prose\n\n\
         ```lua\n\
         tools.bind('echo', {capability})\n\
         tools.always('echo')\n\
         models.default('writer', 'A general model for tests')\n\
         var.executions = (var.executions or 0) + 1\n\
         ```\n\n\
         Ask for one tool call.\n\n\
         ```lua\n\
         var.non_final_had_text = reply ~= nil\n\
         var.executions = var.executions + 1\n\
         ```\n\n\
         Finish now.\n\n\
         ```lua\n\
         var.final_reply = reply\n\
         var.executions = var.executions + 1\n\
         ```\n\n\
         ## Result\n\n\
         ```lua\n\
         return tostring(var.non_final_had_text) .. ':' .. var.final_reply .. ':' .. var.executions\n\
         ```\n"
    );
    let prompt = parse(&md);
    let ctx = h1_context(&prompt);
    let tools: [Arc<dyn Tool>; 1] = [echo];
    let resolution = H1Resolution {
        picker: ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
            .expect("tool picker must build"),
        models: test_model_catalog(),
        tools: ToolCatalog::new(&tools).expect("the fixture tool is unique"),
    };
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("live H1 prose must preserve block semantics");

    assert_eq!(out, "false:final answer:3");
}

#[tokio::test(flavor = "current_thread")]
async fn the_live_h1_pass_fires_no_section_boundaries() {
    // The completion-flag contract on the scheduler: the H1 frame never
    // arms completion and is never a walked section, so the pass reports
    // its teardown pair but neither SECTION_STARTED nor SECTION_FINISHED;
    // the first walked section reports both.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Boundaries\n\n\
        ```lua\nvar.x = 1\n```\n\n\
        ## Only\n\n\
        ```lua\nreturn 'done-now'\n```\n";
    let prompt = parse(md);
    let ctx = h1_context_on(&prompt, &StoreRef::memory(), recorder.clone());
    let resolution = H1Resolution::empty();
    let out = Scheduler::new(&ctx, None)
        .with_live_h1(resolution.context())
        .drive()
        .await
        .expect("the pass and the walk complete");

    assert_eq!(out, "done-now");
    let observed = recorder.events();
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    assert!(
        observed.contains(&("Only".to_string(), started.clone())),
        "the walked section reports started: {observed:?}"
    );
    assert!(
        observed.contains(&("Only".to_string(), finished.clone())),
        "the walked section reports finished: {observed:?}"
    );
    assert!(
        observed
            .iter()
            .any(|(section, event)| section == "Boundaries"
                && event == &detail::LUA_TEARDOWN_SUCCEEDED.to_string()),
        "the H1 frame's drop fires the teardown pair: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(section, event)| section == "Boundaries"
                && (event == &started || event == &finished)),
        "the live H1 pass fires no section boundaries: {observed:?}"
    );
}

// --- Fanout on the scheduler: N arm chains interleaved by the driver ---
// Each mirrored test names the legacy case it mirrors. The legacy cases
// keep exercising the legacy fanout driver untouched; these prove the
// scheduler's arm chains.

/// Builds the run context for a scheduler fanout test with the given
/// limits, so a window test can narrow the concurrency.
fn scheduler_context_with_limits(prompt: &Prompt, limits: RunLimits) -> RunContext {
    let ctx = RunContext::new(
        prompt,
        "",
        &StoreRef::memory(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunConfig::new(EXECUTION).limits(limits),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = writer_models();
    ctx
}

/// The prompt each gateway request carried, in arrival order.
fn request_prompts(gateway: &ScriptedGateway) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| {
            body["messages"][0]["content"]
                .as_str()
                .expect("an infer request carries a user message")
                .to_owned()
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_results_follow_collection_order_not_finish_order() {
    // Mirror of the legacy `results_follow_collection_order_not_finish_order`:
    // arm "two" finishes first (one infer) while arm "one" is still parked
    // on its second; the packed sequence must follow collection order. A
    // join that keyed results by completion order would return "r2|r1:r3".
    let gateway =
        ScriptedGateway::start(vec![resp_text("r1"), resp_text("r2"), resp_text("r3")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local first = models.infer(item .. ':1')\n\
        if item == 'one' then\n\
          return first .. ':' .. models.infer('one:2')\n\
        end\n\
        return first\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "r1:r3|r2");
    assert_eq!(
        request_prompts(&gateway),
        vec!["one:1", "two:1", "one:2"],
        "both arms start before either completes, and arm one finishes last"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_arms_interleave_at_io_points_on_one_thread() {
    // The interleaving proof: both arms reach their first infer before
    // either resumes, so the gateway logs `one:1` then `two:1` - arms
    // driven sequentially would log `one:1`, `one:2` first. Each arm
    // returns its first answer, so the result is deterministic regardless
    // of which arm's second answer lands first.
    let gateway = ScriptedGateway::start(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        local b = models.infer(item .. ':2')\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "r1|r2");
    let prompts = request_prompts(&gateway);
    assert_eq!(prompts.len(), 4, "both arms run both infers: {prompts:?}");
    assert_eq!(
        prompts[..2],
        ["one:1", "two:1"],
        "the second arm reaches I/O before the first arm resumes: {prompts:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_concurrency_window_limits_active_arms() {
    // The window mirror of the legacy `ArmWindow` contract: with the
    // window at 1, each arm runs both of its infers before the next arm
    // starts - a window that let arms overlap would interleave the
    // requests (x:a, y:a, ...).
    let gateway = ScriptedGateway::start(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
        resp_text("r5"),
        resp_text("r6"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'x', 'y', 'z'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':a')\n\
        local b = models.infer(item .. ':b')\n\
        return a .. b\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_with_limits(
        &prompt,
        RunLimits::new().max_fanout_concurrency(NonZeroUsize::new(1).expect("1 is non-zero")),
    );
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the windowed fanout completes on the scheduler");

    assert_eq!(out, "r1r2|r3r4|r5r6");
    assert_eq!(
        request_prompts(&gateway),
        vec!["x:a", "x:b", "y:a", "y:b", "z:a", "z:b"],
        "with the window at 1 each arm finishes before the next starts"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_arms_take_global_ids_per_fanout_index_and_structured_results() {
    // Mirror of the legacy `fanout_arms_take_global_ids_and_per_fanout_index`
    // plus the structured-result shape of `fanout_returns_structured_results`:
    // each arm entry takes the next run-global id, `sys.index` is the
    // 1-based per-fanout position, and the packed sequence carries `.ok`
    // and `.item` with `__tostring` driving `table.concat`.
    let store = StoreRef::memory();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'a', 'b'})\n\
        assert(r[1].ok and r[2].ok, 'both arms succeed')\n\
        assert(r[2].item == 'b', 'the item rides the result object')\n\
        store.append('ids.txt', 'parent:' .. sys.id .. '\\n')\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('ids.txt', sys.id .. ':' .. sys.index .. '\\n')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "a,b");
    assert_eq!(
        store.read("ids.txt").expect("the ids log"),
        "2:1\n3:2\nparent:1\n",
        "the arms take the next run-global ids with their per-fanout index"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_over_a_large_collection_refills_the_window() {
    // Mirror of the legacy `fanout_accepts_a_list_over_the_old_default_cap`:
    // a 1025-member collection runs to completion past the 8-wide default
    // window - a refill that lost track of the next index would stall the
    // driver or drop results.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local items = {}\n\
        for i = 1, 1025 do items[i] = tostring(i) end\n\
        local r = fanout('### Worker', items)\n\
        return #r .. ':' .. r[1].text .. ':' .. r[1025].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let ctx = scheduler_context(&prompt);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a collection over the window width completes");

    assert_eq!(out, "1025:1:1025");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_inside_a_fanout_arm_drives_a_child_walk() {
    // Mirror of the legacy `jump_inside_a_fanout_arm_drives_a_child_walk`:
    // the arm's remaining blocks are skipped, the walk continues on the
    // target's own slice from the target (the run-global id sequence
    // continues, the walk falls through to the target's following
    // siblings), and the walk's reply becomes the arm's text. A
    // `resolve_arm_target` that resolved over the wrong set would error
    // the jump not-found; one that started the walk elsewhere would break
    // the order log.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        jump('### Target')\n\
        error('the arm remaining blocks are skipped')\n\
        ```\n\n\
        ### Target\n\n\
        ```lua\n\
        assert(sys.id == 3, 'the child walk continues the run-global sys.id sequence')\n\
        store.append('order.txt', 'Target\\n')\n\
        ```\n\n\
        ### Tail\n\n\
        ```lua\n\
        store.append('order.txt', 'Tail\\n')\n\
        return 'tail-reply'\n\
        ```\n";
    let store = StoreRef::memory();
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump inside an arm drives a child walk");

    assert_eq!(out, "tail-reply");
    assert_eq!(
        store.read("order.txt").expect("the order log"),
        "Target\nTail\n",
        "the child walk runs the target and falls through to its siblings"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_from_an_arm_to_a_worker_child_walks_the_child_slice() {
    // Mirror of the legacy
    // `jump_inside_a_fanout_arm_to_a_worker_child_walks_the_child_slice`:
    // the descent runs the worker's child slice from the target, the target
    // takes the next run-global id with no `item` seed (the transfer clears
    // the arm's at-worker state, so the child walk runs as plain sections),
    // and the walk falls through to the target's child siblings.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        jump('#### Child')\n\
        error('the arm remaining blocks are skipped')\n\
        ```\n\n\
        #### Child\n\n\
        ```lua\n\
        assert(sys.id == 3, 'the child walk continues the run-global sys.id sequence')\n\
        assert(item == nil, 'the child walk runs as a plain section')\n\
        store.append('order.txt', 'Child\\n')\n\
        ```\n\n\
        #### ChildTail\n\n\
        ```lua\n\
        store.append('order.txt', 'ChildTail\\n')\n\
        return 'child-tail-reply'\n\
        ```\n";
    let store = StoreRef::memory();
    let prompt = parse(md);
    let ctx = scheduler_context_on(&prompt, &store, Arc::new(NullObserver));
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("a jump to a worker child walks the child slice");

    assert_eq!(out, "child-tail-reply");
    assert_eq!(
        store.read("order.txt").expect("the order log"),
        "Child\nChildTail\n",
        "the child walk runs the target and falls through to its child siblings"
    );
}
