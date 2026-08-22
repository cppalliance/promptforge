use std::error::Error as _;
use std::num::NonZeroU32;

use serde_json::json;

use super::arm::ArmFinalizer;
use super::proxies::ProxyObserver;
use super::*;
use crate::cancel::CancelHandle;
use crate::execute::RunLimits;
use crate::lua::LuaProgram;
use crate::observe::{NullObserver, Observer, detail};
use crate::parser::Block;
use crate::store::StoreRef;
use crate::tools::SharedTools;

#[test]
fn resolve_sibling_finds_exact_match() {
    let sections = vec![sibling("Worker", 3), sibling("Topics", 3)];
    let found = resolve_sibling("### Worker", &sections).expect("must resolve");
    assert_eq!(found.name, "Worker");
}

#[test]
fn resolve_sibling_missing_heading_lists_available() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_sibling_bare_name_errors() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    crate::test_support::synthetic_section(
        name,
        level,
        vec![Block::Prose {
            text: String::new(),
            loop_capable: true,
        }],
        Vec::new(),
    )
}

#[test]
fn resolve_sibling_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("###Worker", &sections)
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
}

#[test]
fn resolve_sibling_marker_only_heading_errors_as_nameless() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### ", &sections).expect_err("a marker-only heading must error");
    assert!(err.to_string().contains("has no name"), "error was: {err}");
}

#[test]
fn resolve_sibling_requires_exact_level() {
    let sections = vec![sibling("Worker", 3)];
    // Same name, wrong marker level, must not resolve.
    let err = resolve_sibling("## Worker", &sections)
        .expect_err("a level mismatch must not resolve by name alone");
    assert!(err.to_string().contains("not found"), "error was: {err}");
    // The exact address resolves.
    let ok = resolve_sibling("### Worker", &sections).expect("exact address resolves");
    assert_eq!(ok.name, "Worker");
}

#[test]
fn resolve_sibling_rejects_more_than_one_match() {
    let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
    let err = resolve_sibling("### Worker", &sections)
        .expect_err("two identical siblings must be rejected as ambiguous");
    assert!(err.to_string().contains("ambiguous"), "error was: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_join_failure_preserves_the_join_error_source() {
    // A panicked/aborted arm surfaces as `Error::FanoutArmJoin` that keeps
    // the structured `JoinError` as its `#[source]`, rather than being
    // flattened into an `Error::Lua` string that loses the cause.
    let join_error = tokio::spawn(async { panic!("arm blew up") })
        .await
        .expect_err("a panicking task must produce a JoinError");
    let error = Error::FanoutArmJoin(join_error);
    assert!(
        error.source().is_some(),
        "the JoinError must be preserved as the error source"
    );
    assert!(
        !error.to_string().contains("arm blew up"),
        "the panic payload is not stringified into the outer message"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_arm_maps_to_fanout_arm_join_through_the_select_loop() {
    // The select loop's non-cancellation `JoinError` arm is reachable only
    // when a spawned arm panics, and no production input can panic an arm
    // (every arm-internal failure is a `Result`), so the sentinel item
    // injects the panic (test-only, in `run_one_arm`). The run must surface
    // `Error::FanoutArmJoin` with the `JoinError` preserved as the source.
    let worker = lua_worker("return item");
    let sentinel = super::arm::PANIC_ARM_SENTINEL;
    let items = vec![json!("ok"), json!(sentinel)];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-join-panic-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let error = cancel::scope(CancelHandle::new(), fixture.run(&ctx, &worker, &items))
        .await
        .expect_err("a panicking arm must fail the fanout");
    assert!(
        matches!(error, Error::FanoutArmJoin(_)),
        "expected FanoutArmJoin, got {error}"
    );
    let join_error = error
        .source()
        .expect("the JoinError is preserved as the error source")
        .downcast_ref::<tokio::task::JoinError>()
        .expect("the source is the structured JoinError");
    assert!(
        join_error.is_panic(),
        "the JoinError must carry the arm's panic"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_cancelled_fanout_returns_interrupted() {
    let worker = lua_worker("return item");
    let items = vec![json!("alpha"), json!("beta")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-cancel-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let cancel = CancelHandle::new();
    cancel.cancel();
    let error = cancel::scope(cancel, fixture.run(&ctx, &worker, &items))
        .await
        .expect_err("pre-cancelled fanout must fail");
    assert!(
        matches!(error, Error::Interrupted),
        "expected Interrupted, got {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_arm_aborts_and_drops_blocked_siblings() {
    // FANOUT-003: drive `run_fanout_arms` for real. A fatal arm error returns
    // from the run level (not Interrupted, not a synthetic JoinError), and the
    // queued/blocked siblings are dropped without running - proven by a store
    // side-channel that only the fatal arm ever wrote to.
    let worker = lua_worker(
        "store.append('log.txt', item)\nif item == 'boom' then error('fatal arm error') end\nreturn item",
    );
    // The fatal item is dispatched first; with concurrency 1 the siblings stay
    // queued and must never be spawned once the first arm fails.
    let items = vec![json!("boom"), json!("beta"), json!("gamma")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-fatal-test",
        RunLimits::new().max_fanout_concurrency(NonZeroUsize::new(1).expect("1 is non-zero")),
        Arc::new(NullObserver),
    );

    let error = cancel::scope(CancelHandle::new(), fixture.run(&ctx, &worker, &items))
        .await
        .expect_err("a fatal arm must fail the whole fanout");
    // A genuine arm failure, not cancellation or a synthetic join failure.
    assert!(
        !matches!(error, Error::Interrupted | Error::FanoutArmJoin(_)),
        "expected a fatal arm error, got {error}"
    );
    // Only the fatal arm ran; the blocked siblings were dropped and never
    // executed their prologue.
    let log = fixture
        .store
        .read("log.txt")
        .expect("the fatal arm wrote its item");
    assert!(log.contains("boom"), "the fatal arm ran: {log:?}");
    assert!(
        !log.contains("beta") && !log.contains("gamma"),
        "blocked siblings must not run after a fatal arm: {log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_arms_writing_one_path_fail_with_a_write_race() {
    // Note 74: two arms of one fanout calling `store.write` on the same path
    // is a hard write-write race. The store error surfaces as a Lua error,
    // which is fatal to the arm and aborts the fanout (note 63).
    let worker = lua_worker("store.write('shared.txt', item)\nreturn item");
    let items = vec![json!("alpha"), json!("beta")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-write-race-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let error = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("two arms writing one path must fail the fanout");
    let text = error.to_string();
    assert!(text.contains("write-write race"), "error was: {text}");
    assert!(text.contains("shared.txt"), "error was: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_arms_appending_one_path_succeed() {
    // Note 74: `append` is untracked, so concurrent appends to one path are
    // legal; only the relative order is unspecified.
    let worker = lua_worker("store.append('log.txt', item .. ';')\nreturn item");
    let items = vec![json!("alpha"), json!("beta")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-append-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let results = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("concurrent appends to one path must succeed");
    assert_eq!(results.len(), 2);
    let log = fixture.store.read("log.txt").expect("both arms appended");
    assert!(log.contains("alpha;"), "log was: {log:?}");
    assert!(log.contains("beta;"), "log was: {log:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_arm_rewriting_its_own_path_succeeds() {
    // The registry records (fanout token, arm index); the same arm writing
    // the same path again is a rewrite, not a race.
    let worker = lua_worker(
        "store.write('own.txt', 'first')\nstore.write('own.txt', 'second')\nreturn item",
    );
    let items = vec![json!("only")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-rewrite-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("an arm rewriting its own path must succeed");
    assert_eq!(
        fixture.store.read("own.txt").expect("the arm wrote"),
        "second"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_fanouts_may_write_one_path() {
    // A later fanout carries a fresh write token, so its write overwrites the
    // earlier fanout's registry record instead of racing against it.
    let worker = lua_worker("store.write('seq.txt', item)\nreturn item");
    let fixture = FanoutFixture::new();
    let first = fixture.ctx(
        "fanout-sequential-1",
        RunLimits::new(),
        Arc::new(NullObserver),
    );
    fixture
        .run(&first, &worker, &[json!("one")])
        .await
        .expect("the first fanout must succeed");
    let second = fixture.ctx(
        "fanout-sequential-2",
        RunLimits::new(),
        Arc::new(NullObserver),
    );
    fixture
        .run(&second, &worker, &[json!("two")])
        .await
        .expect("a sequential fanout may write the same path");
    assert_eq!(
        fixture.store.read("seq.txt").expect("both fanouts wrote"),
        "two"
    );
}

#[test]
fn arm_window_never_exceeds_the_concurrency_limit() {
    // Drive the pure scheduler through every completion order for a few
    // sizes and prove the invariant that gates real arms: outstanding never
    // exceeds the limit, and each index is dispatched exactly once.
    for &limit in &[1usize, 2, 3, 5] {
        for &count in &[0usize, 1, 4, 9, 20] {
            let concurrency = NonZeroUsize::new(limit).expect("limit is non-zero");
            let mut window = ArmWindow::new(count, concurrency);
            let mut in_flight: Vec<usize> = Vec::new();
            let mut dispatched: Vec<usize> = Vec::new();

            while let Some(index) = window.take_next() {
                in_flight.push(index);
                dispatched.push(index);
            }
            assert!(
                in_flight.len() <= limit,
                "initial window {} exceeded limit {limit}",
                in_flight.len()
            );
            let mut toggle = false;
            while !in_flight.is_empty() {
                assert!(
                    in_flight.len() <= limit,
                    "outstanding {} exceeded limit {limit}",
                    in_flight.len()
                );
                // Complete arms from alternating ends to vary the order.
                if toggle {
                    in_flight.remove(0);
                } else {
                    in_flight.pop();
                }
                toggle = !toggle;
                window.complete_one();
                while let Some(index) = window.take_next() {
                    in_flight.push(index);
                    dispatched.push(index);
                }
            }

            dispatched.sort_unstable();
            assert_eq!(
                dispatched,
                (0..count).collect::<Vec<_>>(),
                "every item index must be dispatched exactly once"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_accepts_a_list_over_the_old_default_cap() {
    // The item cap is gone; the concurrency window is the only bound. A
    // 1025-member collection (one over the old 1024 default) runs to
    // completion. The worker is pure Lua with an immediate return, so the
    // run needs no client and stays fast.
    let worker = lua_worker("return item");
    let items: Vec<serde_json::Value> = (0..1025).map(|i| json!(i.to_string())).collect();
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-uncapped-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let results = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("a collection over the old default cap must succeed");
    assert_eq!(results.len(), 1025);
    assert_eq!(
        results[1024],
        LuaFanoutResult::success(json!("1024"), "1024"),
        "the last arm's result follows collection order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_required_when_arm_prose_has_no_binding() {
    let mut worker = sibling("Worker", 3);
    worker.blocks = vec![Block::Prose {
        text: "Ask the model about {{ item }}.".to_string(),
        loop_capable: true,
    }];
    let items = vec![json!("alpha")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx("fanout-test", RunLimits::new(), Arc::new(NullObserver));

    let error = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("non-empty arm prose without a model binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model binding required for section Worker"),
        "error must name the worker section: {error}"
    );
}

/// Records every observation it is handed, in order, so tests assert against
/// the `detail::FANOUT_ARM_*` constants rather than Display wording.
#[derive(Default)]
struct EventRecorder(std::sync::Mutex<Vec<Observation>>);

impl Observer for EventRecorder {
    fn observe(&self, _execution: &str, _section: &str, event: Observation) {
        self.0
            .lock()
            .expect("recorder mutex is not poisoned")
            .push(event);
    }
}

impl EventRecorder {
    fn snapshot(&self) -> Vec<Observation> {
        self.0
            .lock()
            .expect("recorder mutex is not poisoned")
            .clone()
    }

    fn count(&self, event: &Observation) -> usize {
        self.snapshot().iter().filter(|e| *e == event).count()
    }

    fn assert_terminal_count(&self, event: &Observation, expected: usize) {
        assert_eq!(
            self.count(event),
            expected,
            "unexpected terminal-event count: {:?}",
            self.snapshot()
        );
    }
}

fn lua_worker(source: &str) -> Section {
    let program = LuaProgram::compile(
        source,
        "test prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        "fanout-terminal-test",
        &crate::observe::NullObserver,
        "Worker",
    )
    .expect("test Lua must compile");
    Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Lua(program)],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    }
}

fn shared_chunk(source: &str) -> LuaProgram {
    LuaProgram::compile(
        source,
        "test shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        "fanout-terminal-test",
        &crate::observe::NullObserver,
        "Worker",
    )
    .expect("test Lua must compile")
}

/// Owns the values every `run_fanout_arms` test threads into a
/// [`RunContext`] plus the call's own inputs, so a test states only its
/// execution name, limits, and observer. Fields stay visible so a test can
/// swap one out (a custom shared library) before building the context.
struct FanoutFixture {
    store: StoreRef,
    shared_tools: SharedTools,
    client: Option<GatewayClient>,
    shared: LuaProgram,
    var: serde_json::Value,
}

impl FanoutFixture {
    fn new() -> Self {
        Self {
            store: StoreRef::memory(),
            shared_tools: SharedTools::default(),
            client: None,
            shared: LuaProgram::empty().expect("the empty chunk compiles"),
            var: json!({}),
        }
    }

    fn ctx(&self, execution: &str, limits: RunLimits, observer: Arc<dyn Observer>) -> RunContext {
        let prompt = crate::parser::Prompt::parse(
            "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n# T\n\n## S\n\ndone\n",
            "fanout-fixture",
            &NullObserver,
        )
        .expect("the fixture prompt parses");
        RunContext::new(
            &prompt,
            "",
            &self.store,
            self.shared_tools.clone(),
            self.shared.clone(),
            &crate::execute::RunConfig::new(execution)
                .limits(limits)
                .observer(observer),
        )
    }

    /// The call's own inputs at their defaults: no client, no reply seed, an
    /// empty home slice, depth zero, and the fixture's `var`.
    async fn run(
        &self,
        ctx: &RunContext,
        worker: &Section,
        items: &[serde_json::Value],
    ) -> Result<Vec<LuaFanoutResult>> {
        run_fanout_arms(
            ctx,
            worker,
            items,
            self.client.as_ref(),
            None,
            &[],
            0,
            &self.var,
        )
        .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_arm_emits_a_distinct_succeeded_terminal_event() {
    // FANOUT-004: every arm is finalized exactly once with a distinct
    // terminal event. Two arms whose prologue returns a value each emit one
    // `started` and one `succeeded`, and nothing else.
    let worker = lua_worker("return item");
    let items = vec![json!("a"), json!("b")];
    let fixture = FanoutFixture::new();
    let recorder = Arc::new(EventRecorder::default());
    let ctx = fixture.ctx("fanout-terminal-test", RunLimits::new(), recorder.clone());

    let results = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("both arms must succeed");
    assert_eq!(results.len(), 2);
    assert_eq!(recorder.count(&detail::FANOUT_ARM_STARTED), 2);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_SUCCEEDED, 2);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_FAILED, 0);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_CANCELLED, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shared_replay_sees_the_arm_item() {
    // The `item` install lands before `replay_shared`, so the shared
    // library's top-level code may read `item`; moving the install after the
    // replay must fail this test.
    let worker = lua_worker("return captured_by_shared");
    let items = vec![json!("alpha")];
    let mut fixture = FanoutFixture::new();
    fixture.shared = shared_chunk("captured_by_shared = item");
    let ctx = fixture.ctx(
        "fanout-terminal-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let results = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("the arm must succeed");
    assert_eq!(
        results,
        vec![LuaFanoutResult::success(json!("alpha"), "alpha")],
        "the shared chunk captured the item before the worker ran"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hard_failing_arm_emits_a_failed_terminal_event() {
    // FANOUT-004: a hard arm error emits a distinct `failed` terminal event,
    // never `succeeded`.
    let worker = lua_worker("error('boom')");
    let items = vec![json!("a")];
    let fixture = FanoutFixture::new();
    let recorder = Arc::new(EventRecorder::default());
    let ctx = fixture.ctx("fanout-terminal-test", RunLimits::new(), recorder.clone());

    fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("a hard arm error must fail the fanout");
    recorder.assert_terminal_count(&detail::FANOUT_ARM_FAILED, 1);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_SUCCEEDED, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vm_construction_failure_emits_one_failed_terminal_event() {
    // The VM-construction-failure branch, covered by test-only fault
    // injection: the finalizer finishes `FANOUT_ARM_FAILED` and the arm
    // returns before its body runs, so the fanout fails with the
    // construction error and no teardown or second terminal event fires.
    let mut worker = lua_worker("return item");
    worker.name = super::arm::FAIL_ARM_VM_SENTINEL.to_string();
    let items = vec![json!("a")];
    let fixture = FanoutFixture::new();
    let recorder = Arc::new(EventRecorder::default());
    let ctx = fixture.ctx("fanout-terminal-test", RunLimits::new(), recorder.clone());

    let error = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("an injected construction failure must fail the fanout");
    assert!(
        error
            .to_string()
            .contains("test-injected VM construction failure"),
        "the construction error propagates: {error}"
    );
    recorder.assert_terminal_count(&detail::FANOUT_ARM_FAILED, 1);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_SUCCEEDED, 0);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_CANCELLED, 0);
    assert_eq!(
        recorder.count(&detail::LUA_TEARDOWN_STARTED),
        0,
        "a failed construction has no VM to tear down: {:?}",
        recorder.snapshot()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_setup_failure_tears_the_vm_down_once_and_emits_one_failed_event() {
    // The setup-error path through the arm's single epilogue: a shared chunk
    // whose top-level errors at replay fails the arm body, so the epilogue
    // tears the VM down exactly once and records one `FANOUT_ARM_FAILED`.
    let worker = lua_worker("return item");
    let items = vec![json!("a")];
    let mut fixture = FanoutFixture::new();
    fixture.shared = shared_chunk("error('boom')");
    let recorder = Arc::new(EventRecorder::default());
    let ctx = fixture.ctx("fanout-terminal-test", RunLimits::new(), recorder.clone());

    let error = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("a shared-replay failure must fail the fanout");
    assert!(
        error.to_string().contains("boom"),
        "the setup error propagates: {error}"
    );
    recorder.assert_terminal_count(&detail::FANOUT_ARM_FAILED, 1);
    recorder.assert_terminal_count(&detail::FANOUT_ARM_SUCCEEDED, 0);
    assert_eq!(
        recorder.count(&detail::LUA_TEARDOWN_STARTED),
        1,
        "the epilogue tears the VM down exactly once: {:?}",
        recorder.snapshot()
    );
    assert_eq!(recorder.count(&detail::LUA_TEARDOWN_SUCCEEDED), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_follow_collection_order_not_finish_order() {
    // The first arm is gated on a store key only the second arm writes, so it
    // finishes after its sibling; the returned vec must still follow
    // collection order, not finish order.
    let worker = lua_worker(
        "if item == 'one' then\n\
            while not pcall(store.read, 'gate.txt') do end\n\
            return 'one-late'\n\
        end\n\
        store.append('gate.txt', 'x')\n\
        return 'two-early'",
    );
    let items = vec![json!("one"), json!("two")];
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-ordering-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let results = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect("both arms must succeed");
    assert_eq!(
        results,
        vec![
            LuaFanoutResult::success(json!("one"), "one-late"),
            LuaFanoutResult::success(json!("two"), "two-early"),
        ],
        "results must follow collection order, not finish order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_collection_is_rejected_before_any_scheduling() {
    // Note 65: no work is likely a bug, so the fanout errors instead of
    // returning zero results.
    let worker = lua_worker("return item");
    let items: Vec<serde_json::Value> = Vec::new();
    let fixture = FanoutFixture::new();
    let ctx = fixture.ctx(
        "fanout-empty-test",
        RunLimits::new(),
        Arc::new(NullObserver),
    );

    let error = fixture
        .run(&ctx, &worker, &items)
        .await
        .expect_err("an empty collection must error");
    assert!(
        matches!(error, Error::Lua(_)),
        "expected Error::Lua, got {error}"
    );
    assert!(
        error.to_string().contains("empty collection"),
        "error was: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_control_globals_resolve_over_the_threaded_home() {
    // The fanout call's home slice reaches the arm's control globals:
    // `list_from_section` resolves a home sibling's items.
    let worker = lua_worker(
        "local items = list_from_section('### List')\n\
        return item .. ':' .. table.concat(items, ',')",
    );
    let items = vec![json!("alpha")];
    let fixture = FanoutFixture::new();
    let mut list = sibling("List", 3);
    list.items = vec!["x".to_string(), "y".to_string()];
    let home = vec![list];
    let ctx = fixture.ctx("fanout-home-test", RunLimits::new(), Arc::new(NullObserver));

    let results = run_fanout_arms(
        &ctx,
        &worker,
        &items,
        fixture.client.as_ref(),
        None,
        &home,
        0,
        &fixture.var,
    )
    .await
    .expect("the arm must succeed");
    assert_eq!(
        results,
        vec![LuaFanoutResult::success(json!("alpha"), "alpha:x,y")],
        "the arm's control globals resolved over the threaded home slice"
    );
}

/// Signals a oneshot the first time it observes a Lua `log` event, so a test
/// can learn deterministically that an arm has started running.
struct SignalOnLog {
    tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl Observer for SignalOnLog {
    fn observe(&self, _execution: &str, _section: &str, event: Observation) {
        if matches!(event, Observation::Lua(_))
            && let Some(tx) = self.tx.lock().expect("signal mutex").take()
        {
            let _ = tx.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_in_flight_fanout_arm_is_cancelled_cooperatively() {
    // PF-CANCEL-002 / FANOUT-003 (in-flight): a spawned arm carries an
    // explicit CancelHandle, so an arm spinning in synchronous Lua stops via
    // its OWN instruction hook when cancelled mid-flight. Without the
    // per-arm handle the arm could not be aborted (synchronous Lua cannot be
    // preempted) and the join drain would hang - so the timeout below is the
    // regression guard. Readiness is signaled explicitly (no sleeps).
    let worker = lua_worker("log('running')\nwhile true do end\nreturn item");
    let items = vec![json!("only")];
    let fixture = FanoutFixture::new();

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let observer = SignalOnLog {
        tx: std::sync::Mutex::new(Some(ready_tx)),
    };
    let ctx = fixture.ctx("fanout-terminal-test", RunLimits::new(), Arc::new(observer));

    let cancel = CancelHandle::new();
    let canceller = {
        let handle = cancel.clone();
        tokio::spawn(async move {
            // Cancel only once the arm has actually started spinning.
            let _ = ready_rx.await;
            handle.cancel();
        })
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cancel::scope(cancel, fixture.run(&ctx, &worker, &items)),
    )
    .await
    .expect("the in-flight arm must cooperatively cancel, not hang the join drain");
    let error = result.expect_err("a cancelled fanout returns an error");
    assert!(
        matches!(error, crate::Error::Interrupted),
        "expected Interrupted, got {error}"
    );
    canceller.await.expect("the canceller task joins");
}

#[test]
fn arm_finalizer_emits_cancelled_on_drop_unless_finished() {
    // FANOUT-004/006: the guard emits exactly one terminal event. Dropped
    // without finishing => cancelled; finished => only that event.
    let (tx, mut rx) = mpsc::channel::<(String, Observation)>(8);
    let observer: Arc<dyn Observer> = Arc::new(ProxyObserver { tx });

    drop(ArmFinalizer::new(
        Arc::clone(&observer),
        "exec".to_string(),
        "S".to_string(),
    ));
    let (_, event) = rx.try_recv().expect("a dropped finalizer emits an event");
    assert_eq!(event, detail::FANOUT_ARM_CANCELLED);
    assert!(rx.try_recv().is_err(), "exactly one terminal event on drop");

    let mut finalizer =
        ArmFinalizer::new(Arc::clone(&observer), "exec".to_string(), "S".to_string());
    finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
    drop(finalizer);
    let (_, event) = rx.try_recv().expect("finish emits its event");
    assert_eq!(event, detail::FANOUT_ARM_SUCCEEDED);
    assert!(
        rx.try_recv().is_err(),
        "a finished finalizer does not also emit cancelled on drop"
    );
}

// --- collection conversion -------------------------------------------------

fn eval(lua: &mlua::Lua, source: &str) -> Value {
    lua.load(source).eval::<Value>().expect("chunk evaluates")
}

#[test]
fn collection_to_items_rejects_a_non_table() {
    let lua = mlua::Lua::new();
    for source in ["return '### Items'", "return 5", "return true"] {
        let value = eval(&lua, source);
        let error = collection_to_items(&lua, &value).expect_err("a non-table is not a collection");
        assert!(
            error.to_string().contains("list_from_section"),
            "the error must point at list_from_section for {source}: {error}"
        );
    }
}

#[test]
fn collection_to_items_preserves_array_order_and_member_types() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'b', 2, true, {nested='x'}}");
    let items = collection_to_items(&lua, &value).expect("a mixed array converts");
    assert_eq!(
        items,
        vec![json!("b"), json!(2), json!(true), json!({"nested": "x"})]
    );
}

#[test]
fn collection_to_items_wraps_hash_members_as_pair_tables() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {alpha=1, beta='two'}");
    let mut items = collection_to_items(&lua, &value).expect("a hash table converts");
    // The hash part's order is undefined; sort for the comparison.
    items.sort_by_key(ToString::to_string);
    assert_eq!(
        items,
        vec![
            json!({"key": "alpha", "value": 1}),
            json!({"key": "beta", "value": "two"})
        ]
    );
}

#[test]
fn collection_to_items_emits_the_array_part_before_the_hash_part() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'a', 'b', extra='c'}");
    let items = collection_to_items(&lua, &value).expect("a mixed table converts");
    assert_eq!(
        items,
        vec![
            json!("a"),
            json!("b"),
            json!({"key": "extra", "value": "c"})
        ]
    );
}

#[test]
fn collection_to_items_keeps_integer_keys_outside_the_border_as_pairs() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {[5]='five'}");
    let items = collection_to_items(&lua, &value).expect("a sparse table converts");
    assert_eq!(items, vec![json!({"key": 5, "value": "five"})]);
}

#[test]
fn collection_to_items_returns_an_empty_vec_for_an_empty_table() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {}");
    let items = collection_to_items(&lua, &value).expect("an empty table converts");
    assert!(items.is_empty());
}

#[test]
fn collection_to_items_rejects_a_function_member_naming_its_index() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'a', function() end}");
    let error = collection_to_items(&lua, &value).expect_err("a function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 2"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");

    let value = eval(&lua, "return {cb=function() end}");
    let error =
        collection_to_items(&lua, &value).expect_err("a hash-position function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index cb"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");
}

struct Stub;
impl mlua::UserData for Stub {}

#[test]
fn collection_to_items_rejects_a_userdata_member_naming_its_index() {
    let lua = mlua::Lua::new();
    let userdata = lua.create_userdata(Stub).expect("userdata creates");
    let table = lua.create_table().expect("table creates");
    table.raw_set(1, userdata).expect("member installs");
    let error =
        collection_to_items(&lua, &Value::Table(table)).expect_err("a userdata member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 1"), "error was: {rendered}");
    assert!(rendered.contains("userdata"), "error was: {rendered}");
}

#[test]
fn collection_to_items_rejects_a_non_scalar_key() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "local t = {}; t[{}] = 'x'; return t");
    let error = collection_to_items(&lua, &value).expect_err("a table key must error");
    assert!(
        error
            .to_string()
            .contains("key must be a string, number, or boolean"),
        "error was: {error}"
    );
}
