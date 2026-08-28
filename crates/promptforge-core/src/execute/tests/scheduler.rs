//! Scheduler-side tests: the decision-gate scenario (nested execute plus
//! inference end-to-end on a current-thread runtime), cancellation while
//! suspended on an infer, the per-chain execute-depth cap, and the walk
//! rules mirrored from the legacy suite (fall-through order, off-walk
//! skips, reply roll-forward, `var` discipline, the run-global id
//! counter), plus the control-transfer rules: jump targets (sibling moves
//! and child descents with the parent resuming after the jumper), the
//! scalar return's chain scoping, and the section-boundary observations.

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
