//! Fanout failure, cancel, and script tool-arm cases for the scheduler.

use std::num::NonZeroUsize;

use super::*;
use crate::execute::run::{EffectAnswer, EffectId};
use crate::test_support::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn two_arms_appending_one_path_boom_without_any_other_suspension() {
    // The store operation alone is the interleaving point now: every store
    // op is a leaf yield, so the arms park live on their appends and the
    // cross-arm append booms. Which arm's op executes first is the
    // blocking pool's choice, and an op that runs to completion lets its
    // arm finish and release its claims - so the test cannot rely on the
    // second op starting while the first is still in flight. The gate
    // parks the first op to reach the backend with its write claim held,
    // and the second op's claim check meets that standing claim no matter
    // how late its thread starts; the conflict's failed observation then
    // opens the gate, so the winner's op completes ahead of the run-end
    // drain that awaits it.
    let gate = Arc::new(StoreGate::default());
    let store = gated_store(&gate);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. ';')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(
        &prompt,
        &store,
        GateObserver::new(&gate, Arc::new(NullObserver::default())),
    );
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("concurrent appends to one path must boom");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("log.txt"), "error was: {detail}");
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
    // The losing arm's append never reached the backend: exactly one arm's
    // append landed.
    let log = store.read("log.txt").expect("one arm appended");
    assert!(
        log == "alpha;" || log == "beta;",
        "exactly one arm's append may land: {log:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_arm_rewriting_its_own_path_succeeds() {
    // Mirror of the legacy case of the same name: the registry records
    // (fanout token, arm index), so the same arm writing the same path
    // again is a rewrite, not a race.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'only'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('own.txt', 'first')\n\
        store.write('own.txt', 'second')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("an arm rewriting its own path must succeed");

    assert_eq!(out, "only");
    assert_eq!(store.read("own.txt").expect("the arm wrote"), "second");
}

#[tokio::test(flavor = "current_thread")]
async fn sequential_fanouts_may_write_one_path() {
    // Mirror of the legacy case of the same name: a later fanout takes a
    // fresh write token, so its write overwrites the earlier fanout's
    // registry record instead of racing against it.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local a = fanout('### Worker', {'one'})\n\
        local b = fanout('### Worker', {'two'})\n\
        return b[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('seq.txt', item)\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a sequential fanout may write the same path");

    assert_eq!(out, "two");
    assert_eq!(store.read("seq.txt").expect("both fanouts wrote"), "two");
}

#[tokio::test(flavor = "current_thread")]
async fn fatal_arm_aborts_queued_siblings() {
    // Mirror of the legacy `fatal_arm_aborts_and_drops_blocked_siblings`:
    // with the window at 1 the siblings stay queued, and once the first
    // arm fails fatally they are never created - proven by the store
    // side-channel only the fatal arm ever wrote to, and by the terminal
    // observations: one FAILED, nothing else.
    let store = TestStore::new();
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {'boom', 'beta', 'gamma'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. '\\n')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_from(
        &prompt,
        &store,
        &test_context(EXECUTION).limits(
            RunLimits::new().max_fanout_concurrency(NonZeroUsize::new(1).expect("1 is non-zero")),
        ),
        RunHost::new().observer(recorder.clone()),
    );
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a fatal arm must fail the whole fanout");

    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    let log = store.read("log.txt").expect("the fatal arm wrote its item");
    assert_eq!(log, "boom\n", "blocked siblings must never run: {log:?}");
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        1,
        "only the fatal arm was ever started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fatal_arm_aborts_an_in_flight_sibling() {
    // The sibling-abort port: the failing arm and a sibling parked on a
    // slow infer are both live when the failure lands. The fanout shim
    // cancels the live sibling before it re-raises, so the cancel removes
    // the sibling from the pending table, aborts its I/O task, and fires
    // the sibling's TaskCancelled BEFORE the parent's chunk failure - a
    // shim that raised first would leak the sibling into `tasks_live`.
    // The 30-second sibling answer and the timeout guard prove the driver
    // never waits on the aborted arm.
    let gateway = ScriptedGateway::start(vec![
        resp_text("boom-answer"),
        resp_delayed_text("slow-answer", std::time::Duration::from_secs(30)),
    ])
    .await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {'boom', 'slow'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr()))).drive(),
    )
    .await
    .expect("the aborted sibling must not stall the driver");
    let error = result.expect_err("a fatal arm must fail the whole fanout");

    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    assert!(
        error.to_string().contains("fatal arm error"),
        "the arm's own error surfaces: {error}"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        1,
        "the in-flight sibling reports cancelled: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    let events = recorder.events();
    let cancelled_at = events
        .iter()
        .position(|(_, event)| event == TASK_CANCELLED)
        .expect("the sibling's cancelled event fired");
    let parent_failed_at = events
        .iter()
        .position(|(section, event)| {
            section == "Parent" && event == &detail::LUA_CHUNK_FAILED.to_string()
        })
        .expect("the parent's chunk failed on the fanout error");
    assert!(
        cancelled_at < parent_failed_at,
        "the sibling abort precedes the parent's resume with the error: {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_caught_fanout_failure_lets_the_caller_continue() {
    // The fanout error is the call's answer resumed through the envelope,
    // so an author `pcall` catches it exactly as on the legacy callback
    // path; the run then continues - including past a stale answer the
    // aborted sibling's already-completed I/O task may have posted, which
    // the driver must discard rather than fail on.
    let gateway = ScriptedGateway::start(vec![
        resp_text("boom-answer"),
        resp_text("slow-answer"),
        resp_text("after-answer"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(fanout, '### Worker', {'boom', 'slow'})\n\
        assert(not ok, 'the fatal arm error reaches the caller')\n\
        local a = models.infer('after')\n\
        return 'caught:' .. a\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the caught fanout failure lets the caller continue");

    assert_eq!(out, "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["boom:1", "slow:1", "after"],
        "both arms dispatched before the failure, then the caller's own infer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_while_suspended_in_a_fanout_arm_interrupts_the_run() {
    // Cancellation while suspended in an arm: both arms are parked on slow
    // infers when the cancel lands, so the driver aborts the in-flight I/O
    // tasks and fails the run with Error::Interrupted. The arms are task
    // chains stranded by the run's end: they started, and the run's end
    // settles each with one `abandoned` terminal naming the run's end -
    // not a cancel or a failure of the arm's own. The 30-second answers
    // and the timeout guard prove the aborted I/O is never awaited.
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn models.infer('hang ' .. item)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let mut driver = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    let canceller = driver.cancel_handle();
    let calls = Arc::clone(&gateway.calls);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), driver.drive())
        .await
        .expect("cancellation must not wait on the aborted in-flight I/O");

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling suspended arms must interrupt the run, got {result:?}"
    );
    assert_eq!(
        gateway.call_count(),
        2,
        "both arms were suspended on their infers when the cancel landed"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED) + terminal_count(&recorder, TASK_FAILED),
        0,
        "a stranded arm reports no cancel or failure of its own: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_ABANDONED_BY_RUN_END),
        2,
        "the run's end settles each stranded arm once: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_spawn_failure_mid_window_cancels_the_started_arms() {
    // With the chain-count bound shrunk so the second arm's spawn fails
    // while the shim fills its window, the shim must cancel the arm it
    // already started before it re-raises: the parent catches the error
    // exactly once, the started arm never runs its block (its chain gets
    // at most the one step that enters its section before the spawner's
    // cancel aborts it), and nothing is left live for the chain-end leak
    // check. The second arm never reaches the arena, so only one
    // TaskStarted fires.
    let gateway = ScriptedGateway::start(vec![resp_text("after-answer")]).await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(fanout, '### Worker', {'one', 'two', 'three'})\n\
        assert(not ok, 'the refill failure reaches the caller')\n\
        return 'caught:' .. models.infer('after')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn 'worked:' .. item\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    // The root walk chain is id 0 and the first arm id 1; the second arm's
    // start trips the bound.
    scheduler.set_max_chains_for_test(2);
    let out = scheduler
        .drive()
        .await
        .expect("the caught spawn failure lets the caller continue");

    assert_eq!(out, "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["after"],
        "no arm ran an infer; only the caller's own request fired"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        1,
        "only the first arm reached the arena: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        1,
        "the shim cancels the started arm before it re-raises: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm ran to completion: {:?}",
        recorder.events()
    );
    assert!(
        !recorder
            .events()
            .iter()
            .any(|(section, event)| section == "Worker" && event == "Lua chunk started"),
        "the cancelled arm never ran its block: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_for_an_unknown_request_id_fails_loudly() {
    // An answer arriving for an id the run never issued (and that is not an
    // orphan) means the host lost track of its effects: the run must fail
    // with Error::Internal rather than silently discard the answer. Only an
    // orphaned id (a fatal sibling's late I/O answer, covered by
    // `a_caught_fanout_failure_lets_the_caller_continue`) may be
    // discarded.
    let gateway = ScriptedGateway::start(vec![resp_text("real-answer")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Infer\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('ask')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    // Handed to the run before the drive: the phantom answer lands ahead
    // of the real infer's, on a run that has issued nothing.
    scheduler
        .run_for_test()
        .resume(EffectId(u64::MAX), EffectAnswer::Timer);
    let error = scheduler
        .drive()
        .await
        .expect_err("an answer for an unissued effect must fail the run");

    assert!(
        matches!(error, Error::Internal { message, .. } if message.contains("did not issue")),
        "the unknown answer is a loud invariant failure: {error}"
    );
}

// --- Script-initiated tools.call dispatch ---

/// Arms the run's shared tool set with `bindings`, every alias in the
/// prompt-wide `always` scope, so a section's effective scope includes them
/// without an H1 pass; the implementations go to the driver's host table.
fn arm_tool_set(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
) -> RunHost {
    arm_tools(ctx, host, bindings)
}

/// Arms the run's shared tool set with `bindings` and exactly `always` as
/// the prompt-wide scope, so a binding can sit in the document catalog
/// without entering any section's effective scope.
fn arm_tool_set_scoped(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
    always: Vec<String>,
) -> RunHost {
    arm_tools_scoped(ctx, host, bindings, always)
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_dispatches_and_resumes_as_a_string() {
    // The whole script path in one pass: the shim yields, the scheduler
    // dispatches the bound tool, the plain binding resumes as a Lua
    // string, and the counts land in the same `tools.calls` table the
    // prose loop feeds.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local out = tools.call('echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls.echo)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the script dispatch succeeds");
    assert_eq!(out, "echoed: hi|1");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_with_a_tool_object_dispatches_its_binding() {
    // The handle form: the captured alias global is an inspectable Tool
    // object, and passing it as the leading argument dispatches the binding
    // it names, identically to the bare alias string.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        assert(type(echo) == 'userdata', 'the captured alias is a Tool object')\n\
        return tools.call(echo, { value = 'hi' })\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the handle-form dispatch succeeds");
    assert_eq!(out, "echoed: hi");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_with_an_unbound_alias_names_the_bound_set() {
    // Script-initiated resolution runs against the run's full bound
    // catalog, so the unknown-alias error names that whole set, not the
    // section's effective scope.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('missing', {})\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("an unbound alias fails the block");
    match &error {
        Error::UnboundToolCall { name, bound } => {
            assert_eq!(name, "missing");
            assert_eq!(bound, &["echo".to_owned()]);
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_reaches_a_bound_tool_outside_the_section_scope() {
    // A tool bound in the document catalog but never scoped into the
    // section (no `always`, no `tools.add`) still dispatches for a script:
    // the scope shapes what the model is offered, and the author's own
    // code is not the model. The count lands in the same shared map
    // `tools.calls` reads.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local out = tools.call('echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls.echo)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set_scoped(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
        Vec::new(),
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a bound but unscoped alias dispatches for a script");
    assert_eq!(out, "echoed: hi|1");
}

/// A tool that signals its start and then never completes, so the
/// cancellation test fires only once the dispatch is in flight.
struct SignallingSlowTool {
    started: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TestTool for SignallingSlowTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/slow").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "slow"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "a deliberately slow tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(
        &self,
        _args: serde_json::Value,
    ) -> std::result::Result<crate::tools::ToolOutput, crate::tools::ToolError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_interrupts_a_slow_script_tools_call() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('slow', {})\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let started = Arc::new(AtomicUsize::new(0));
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding(
            "slow",
            "slow tool",
            Arc::new(SignallingSlowTool {
                started: Arc::clone(&started),
            }),
        )],
    );
    let mut driver = TokioDriver::new(&ctx, host, None);
    let canceller = driver.cancel_handle();
    let observed = Arc::clone(&started);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while observed.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let start = std::time::Instant::now();
    let result = driver.drive().await;

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling a suspended tools.call must interrupt the run, got {result:?}"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "the cancellation must land after the tool call was in flight"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "the slow tool must not hold the run, took {:?}",
        start.elapsed()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_untrusted_script_tools_call_result_is_nonce_wrapped() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('fetch', { value = 'hi' })\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding(
            "fetch",
            "untrusted echo tool",
            Arc::new(UntrustedEchoTool),
        )],
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the untrusted dispatch succeeds");
    assert!(
        out.contains("<untrusted_input_") && out.contains("</untrusted_input_"),
        "the script must receive the nonce-wrapped envelope, got: {out}"
    );
    assert!(
        out.contains("echoed: hi"),
        "the wrapped block must still include the tool output, got: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_structured_binding_resumes_as_a_lua_table() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local r = tools.call('form', {})\n\
        return r.text .. '|' .. tostring(#r.images)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "{\"text\":\"typed\",\"images\":[]}",
            trusted: true,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let host = arm_tool_set(&ctx, host, vec![binding]);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the structured dispatch succeeds");
    assert_eq!(out, "typed|0");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_json_from_a_structured_tool_is_a_tool_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('form', {})\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "not json",
            trusted: true,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let host = arm_tool_set(&ctx, host, vec![binding]);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("invalid structured output fails the call");
    match &error {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the tool error names the invalid JSON, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_untrusted_structured_output_is_wrapped_before_classification() {
    // The untrusted nonce wrap precedes the structured JSON parse, so an
    // untrusted binding's valid JSON still fails the call: this ordering is
    // what restricts structured output to trusted tools. If classification
    // ever ran on the raw output, this test would resume a table and fail.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('form', {})\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "{\"text\":\"typed\"}",
            trusted: false,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let host = arm_tool_set(&ctx, host, vec![binding]);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("untrusted structured output fails the call");
    match &error {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the wrap must precede the parse, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_before_infer_keeps_the_model_install() {
    // The one-time section scope install is shared between the first script
    // dispatch and the model resolution: a script `tools.call` that runs
    // first must not swallow the install a later `models.infer` relies on.
    let gateway = ScriptedGateway::start(vec![resp_text("prose answer")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.call('echo', { value = 'x' })\n```\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("infer after a script dispatch still resolves the model");
    assert_eq!(out, "prose answer");
}

#[tokio::test(flavor = "current_thread")]
async fn a_document_prompt_without_tools_call_is_unaffected() {
    // Bindings installed, shim present, `tools.call` never called: the
    // section runs exactly as before the dispatch arm existed.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn 'plain'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let host = arm_tool_set(
        &ctx,
        host,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a prompt that never calls tools.call is unchanged");
    assert_eq!(out, "plain");
}
