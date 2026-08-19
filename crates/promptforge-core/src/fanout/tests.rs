use std::num::NonZeroU32;

use serde_json::json;

use super::arm::ArmFinalizer;
use super::proxies::ProxyObserver;
use super::*;
use crate::observe::detail;
use crate::parser::Block;

#[test]
fn resolve_sibling_finds_exact_match() {
    let sections = vec![
        Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: String::new(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
            off_walk: false,
        },
        Section {
            name: "Topics".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: String::new(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: vec!["a".to_string()],
            off_walk: false,
        },
    ];
    let found = resolve_sibling("### Worker", &sections).expect("must resolve");
    assert_eq!(found.name, "Worker");
}

#[test]
fn resolve_sibling_missing_heading_lists_available() {
    let sections = vec![Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Prose {
            text: String::new(),
            loop_capable: true,
        }],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    }];
    let err = resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_sibling_bare_name_errors() {
    let sections = vec![Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Prose {
            text: String::new(),
            loop_capable: true,
        }],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    }];
    let err = resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    Section {
        name: name.to_string(),
        level,
        blocks: vec![Block::Prose {
            text: String::new(),
            loop_capable: true,
        }],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    }
}

#[test]
fn resolve_sibling_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("###Worker", &sections)
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
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
    use std::error::Error as _;

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
async fn pre_cancelled_fanout_returns_interrupted() {
    use crate::Error;
    use crate::cancel::{self, CancelHandle};
    use crate::client::GatewayClient;
    use crate::lua::LuaProgram;
    use crate::model::ModelBindings;
    use crate::observe::NullObserver;
    use crate::parser::Section;
    use crate::store::StoreRef;

    let prologue = LuaProgram::compile(
        "return item",
        "test prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        "fanout-cancel-test",
        &NullObserver,
        "Worker",
    )
    .expect("test Lua must compile");
    let worker = Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Lua(prologue)],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    };
    let items = vec![json!("alpha"), json!("beta")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let observer = NullObserver;
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = FanoutContext {
        args: "",
        store: &store,
        execution: "fanout-cancel-test",
        observer: &observer,
        client: &client,
        debug: None,
        shared: &shared,
        bindings: &bindings,
        models: &models,
        analysis: &analysis,
        shared_tools: &shared_tools,
        max_tool_iterations: 24,
        fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
        max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
        lua_memory_bytes: 64 * 1024 * 1024,
        lua_log_events: 1024,
        last_reply: None,
        when: "2026-08-08",
        parent_id: 1,
        section_count: 1,
    };

    let cancel = CancelHandle::new();
    cancel.cancel();
    let error = cancel::scope(cancel, run_fanout_arms(&worker, &items, &ctx))
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
    use crate::cancel::{self, CancelHandle};
    use crate::client::GatewayClient;
    use crate::model::ModelBindings;
    use crate::observe::NullObserver;
    use crate::parser::Section;
    use crate::store::StoreRef;

    let prologue = LuaProgram::compile(
        "store.append('log.txt', item)\nif item == 'boom' then error('fatal arm error') end\nreturn item",
        "worker prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        "fanout-fatal-test",
        &NullObserver,
        "Worker",
    )
    .expect("test Lua must compile");
    let worker = Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Lua(prologue)],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    };
    // The fatal item is dispatched first; with concurrency 1 the siblings stay
    // queued and must never be spawned once the first arm fails.
    let items = vec![json!("boom"), json!("beta"), json!("gamma")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let observer = NullObserver;
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = FanoutContext {
        args: "",
        store: &store,
        execution: "fanout-fatal-test",
        observer: &observer,
        client: &client,
        debug: None,
        shared: &shared,
        bindings: &bindings,
        models: &models,
        analysis: &analysis,
        shared_tools: &shared_tools,
        max_tool_iterations: 24,
        fanout_concurrency: NonZeroUsize::new(1).expect("1 is non-zero"),
        max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
        lua_memory_bytes: 64 * 1024 * 1024,
        lua_log_events: 1024,
        last_reply: None,
        when: "2026-08-08",
        parent_id: 1,
        section_count: 1,
    };

    let error = cancel::scope(CancelHandle::new(), run_fanout_arms(&worker, &items, &ctx))
        .await
        .expect_err("a fatal arm must fail the whole fanout");
    // A genuine arm failure, not cancellation or a synthetic join failure.
    assert!(
        !matches!(error, Error::Interrupted | Error::FanoutArmJoin(_)),
        "expected a fatal arm error, got {error}"
    );
    // Only the fatal arm ran; the blocked siblings were dropped and never
    // executed their prologue.
    let log = store.read("log.txt").expect("the fatal arm wrote its item");
    assert!(log.contains("boom"), "the fatal arm ran: {log:?}");
    assert!(
        !log.contains("beta") && !log.contains("gamma"),
        "blocked siblings must not run after a fatal arm: {log:?}"
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
            let mut max_outstanding = 0usize;

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
                max_outstanding = max_outstanding.max(in_flight.len());
                // Complete arms from alternating ends to vary the order.
                let done = if toggle {
                    in_flight.remove(0)
                } else {
                    in_flight.pop().expect("non-empty")
                };
                toggle = !toggle;
                let _ = done;
                window.complete_one();
                while let Some(index) = window.take_next() {
                    in_flight.push(index);
                    dispatched.push(index);
                }
            }

            assert!(max_outstanding <= limit);
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
async fn fanout_rejects_a_list_over_the_item_cap() {
    use crate::client::GatewayClient;
    use crate::model::ModelBindings;
    use crate::observe::NullObserver;
    use crate::parser::Section;
    use crate::store::StoreRef;

    let worker = Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Prose {
            text: "irrelevant".to_string(),
            loop_capable: true,
        }],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    };
    let items: Vec<serde_json::Value> = (0..5).map(|i| json!(i.to_string())).collect();
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let observer = NullObserver;
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = FanoutContext {
        args: "",
        store: &store,
        execution: "fanout-cap-test",
        observer: &observer,
        client: &client,
        debug: None,
        shared: &shared,
        bindings: &bindings,
        models: &models,
        analysis: &analysis,
        shared_tools: &shared_tools,
        max_tool_iterations: 24,
        fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
        max_fanout_items: NonZeroUsize::new(3).expect("3 is non-zero"),
        lua_memory_bytes: 64 * 1024 * 1024,
        lua_log_events: 1024,
        last_reply: None,
        when: "2026-08-08",
        parent_id: 1,
        section_count: 1,
    };

    let error = run_fanout_arms(&worker, &items, &ctx)
        .await
        .expect_err("a list longer than max_fanout_items must be rejected");
    assert!(
        error.to_string().contains("exceeding the maximum of 3"),
        "error must explain the item cap: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_required_when_arm_prose_has_no_binding() {
    use crate::Error;
    use crate::client::GatewayClient;
    use crate::model::ModelBindings;
    use crate::observe::NullObserver;
    use crate::parser::Section;
    use crate::store::StoreRef;

    let worker = Section {
        name: "Worker".to_string(),
        level: 3,
        blocks: vec![Block::Prose {
            text: "Ask the model about {{ item }}.".to_string(),
            loop_capable: true,
        }],
        children: Vec::new(),
        items: Vec::new(),
        off_walk: false,
    };
    let items = vec![json!("alpha")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let observer = NullObserver;
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = FanoutContext {
        args: "",
        store: &store,
        execution: "fanout-test",
        observer: &observer,
        client: &client,
        debug: None,
        shared: &shared,
        bindings: &bindings,
        models: &models,
        analysis: &analysis,
        shared_tools: &shared_tools,
        max_tool_iterations: 24,
        fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
        max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
        lua_memory_bytes: 64 * 1024 * 1024,
        lua_log_events: 1024,
        last_reply: None,
        when: "2026-08-08",
        parent_id: 1,
        section_count: 1,
    };

    let error = run_fanout_arms(&worker, &items, &ctx)
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

/// Records every observation's Display string, in order.
#[derive(Default)]
struct EventRecorder(std::sync::Mutex<Vec<String>>);

impl Observer for EventRecorder {
    fn observe(&self, _execution: &str, _section: &str, event: Observation) {
        self.0
            .lock()
            .expect("recorder mutex is not poisoned")
            .push(event.to_string());
    }
}

impl EventRecorder {
    fn snapshot(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("recorder mutex is not poisoned")
            .clone()
    }

    fn count(&self, label: &str) -> usize {
        self.snapshot()
            .iter()
            .filter(|e| e.as_str() == label)
            .count()
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

#[expect(
    clippy::ref_option,
    clippy::too_many_arguments,
    reason = "FanoutContext.client borrows an Option<GatewayClient>, so the helper must too; the argument list mirrors the context's fields"
)]
fn terminal_ctx<'a>(
    observer: &'a dyn Observer,
    store: &'a StoreRef,
    bindings: &'a ToolBindings,
    models: &'a ModelBindings,
    analysis: &'a crate::execute::ToolAnalysis,
    shared_tools: &'a SharedTools,
    client: &'a Option<GatewayClient>,
    shared: &'a LuaProgram,
) -> FanoutContext<'a> {
    FanoutContext {
        args: "",
        store,
        execution: "fanout-terminal-test",
        observer,
        client,
        debug: None,
        shared,
        bindings,
        models,
        analysis,
        shared_tools,
        max_tool_iterations: 24,
        fanout_concurrency: NonZeroUsize::new(4).expect("4 is non-zero"),
        max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
        lua_memory_bytes: 64 * 1024 * 1024,
        lua_log_events: 1024,
        last_reply: None,
        when: "2026-08-08",
        parent_id: 1,
        section_count: 1,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_arm_emits_a_distinct_succeeded_terminal_event() {
    // FANOUT-004: every arm is finalized exactly once with a distinct
    // terminal event. Two arms whose prologue returns a value each emit one
    // `started` and one `succeeded`, and nothing else.
    let worker = lua_worker("return item");
    let items = vec![json!("a"), json!("b")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let recorder = EventRecorder::default();
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = terminal_ctx(
        &recorder,
        &store,
        &bindings,
        &models,
        &analysis,
        &shared_tools,
        &client,
        &shared,
    );

    let results = run_fanout_arms(&worker, &items, &ctx)
        .await
        .expect("both arms must succeed");
    assert_eq!(results.len(), 2);
    assert_eq!(recorder.count("Fanout arm started"), 2);
    assert_eq!(
        recorder.count("Fanout arm succeeded"),
        2,
        "each arm emits one distinct succeeded event: {:?}",
        recorder.snapshot()
    );
    assert_eq!(recorder.count("Fanout arm failed"), 0);
    assert_eq!(recorder.count("Fanout arm cancelled"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hard_failing_arm_emits_a_failed_terminal_event() {
    // FANOUT-004: a hard arm error emits a distinct `failed` terminal event,
    // never `succeeded`.
    let worker = lua_worker("error('boom')");
    let items = vec![json!("a")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;
    let recorder = EventRecorder::default();
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = terminal_ctx(
        &recorder,
        &store,
        &bindings,
        &models,
        &analysis,
        &shared_tools,
        &client,
        &shared,
    );

    run_fanout_arms(&worker, &items, &ctx)
        .await
        .expect_err("a hard arm error must fail the fanout");
    assert_eq!(
        recorder.count("Fanout arm failed"),
        1,
        "the failing arm emits one failed event: {:?}",
        recorder.snapshot()
    );
    assert_eq!(recorder.count("Fanout arm succeeded"), 0);
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
    use crate::cancel::{self, CancelHandle};

    let worker = lua_worker("log('running')\nwhile true do end\nreturn item");
    let items = vec![json!("only")];
    let store = StoreRef::memory();
    let bindings = ToolBindings::default();
    let models = <ModelBindings as Default>::default();
    let analysis = crate::execute::ToolAnalysis::default();
    let shared_tools = SharedTools::default();
    let client: Option<GatewayClient> = None;

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let observer = SignalOnLog {
        tx: std::sync::Mutex::new(Some(ready_tx)),
    };
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
    let ctx = terminal_ctx(
        &observer,
        &store,
        &bindings,
        &models,
        &analysis,
        &shared_tools,
        &client,
        &shared,
    );

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
        cancel::scope(cancel, run_fanout_arms(&worker, &items, &ctx)),
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
    let proxy = Arc::new(ProxyObserver { tx });

    drop(ArmFinalizer::new(
        Arc::clone(&proxy),
        "exec".to_string(),
        "S".to_string(),
    ));
    let (_, event) = rx.try_recv().expect("a dropped finalizer emits an event");
    assert_eq!(event.to_string(), "Fanout arm cancelled");
    assert!(rx.try_recv().is_err(), "exactly one terminal event on drop");

    let mut finalizer = ArmFinalizer::new(Arc::clone(&proxy), "exec".to_string(), "S".to_string());
    finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
    drop(finalizer);
    let (_, event) = rx.try_recv().expect("finish emits its event");
    assert_eq!(event.to_string(), "Fanout arm succeeded");
    assert!(
        rx.try_recv().is_err(),
        "a finished finalizer does not also emit cancelled on drop"
    );
}
