//! Scheduler-side tests: the decision-gate scenario (nested execute plus
//! inference end-to-end on a current-thread runtime), cancellation while
//! suspended on an infer, the per-chain execute-depth cap, and the core
//! walk rules mirrored from the legacy suite (fall-through order, off-walk
//! skips, reply roll-forward, `var` discipline, the run-global id
//! counter).

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
