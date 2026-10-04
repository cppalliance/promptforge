//! The worker: FIFO draining, the active command's cancellation and busy
//! text, shutdown and wakeups, and the command bodies it runs.

use std::sync::PoisonError;

use super::*;

#[tokio::test]
async fn the_worker_drains_the_queue_in_fifo_order() {
    let state = state();
    let queue = state.commands.clone();
    let order = Arc::new(Mutex::new(Vec::new()));
    let executor: Arc<Executor> = Arc::new({
        let order = Arc::clone(&order);
        move |_state, command: Command, _activity| {
            let order = Arc::clone(&order);
            Box::pin(async move {
                let label = command.label();
                order
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(label.clone());
                Ok(label)
            }) as BoxFuture<'static, Outcome>
        }
    });
    let worker = state
        .commands
        .spawn_worker_with(&state, executor)
        .expect("worker spawns");

    let a = queue.enqueue(unload("a"));
    let b = queue.enqueue(unload("b"));
    let c = queue.enqueue(unload("c"));
    for handle in [a, b, c] {
        let outcome = handle.outcome.await.expect("each command settles");
        assert!(outcome.is_ok(), "the stub body succeeds: {outcome:?}");
    }
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");

    assert_eq!(
        order
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        ["unload-model: a", "unload-model: b", "unload-model: c"],
        "one worker drains the channel in FIFO order"
    );
    assert!(queue.active_command().is_none(), "the queue is idle");
}

#[tokio::test]
async fn cancel_active_fires_the_active_commands_token() {
    let state = state();
    let queue = state.commands.clone();
    let _worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");

    assert!(!queue.cancel_active(), "no active command to cancel");
    let handle = queue.enqueue(load_profile("alpha"));
    wait_until("the command to go active", || {
        queue.active_command().is_some()
    })
    .await;
    assert_eq!(
        queue.active_command().expect("active").name,
        "load-profile: alpha"
    );

    assert!(queue.cancel_active());
    let outcome = handle.outcome.await.expect("the command settles");
    assert!(
        matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
        "the parked command observes its token: {outcome:?}"
    );
    wait_until("the queue to go idle", || queue.active_command().is_none()).await;
    queue.shutdown();
}

/// The worker begins the command's activity under its label, the body
/// refines the text, and the hub falls idle once the body returns.
#[tokio::test]
async fn the_active_command_drives_the_hubs_busy_text() {
    let state = state();
    let queue = state.commands.clone();
    let hub = Arc::clone(&state.hub);
    assert!(!hub.current().busy, "a pending command is not yet busy");
    let _pending = queue.enqueue(load_profile("alpha"));
    assert!(
        !hub.current().busy,
        "a queued command that has not started reports nothing"
    );

    // The stub writes a stage into the activity, then parks until
    // cancelled.
    let executor: Arc<Executor> = Arc::new(|_state, command, activity| {
        Box::pin(async move {
            activity.set_text("Downloading qwen 45%");
            let label = command.label();
            let token = command.token().expect("a load command holds a token");
            token.cancelled().await;
            Err(GatewayError::CommandCancelled(label))
        }) as BoxFuture<'static, Outcome>
    });
    let _worker = state
        .commands
        .spawn_worker_with(&state, executor)
        .expect("worker spawns");

    wait_until("the command to write its stage", || {
        hub.current().text == "Downloading qwen 45%"
    })
    .await;
    assert!(hub.current().busy, "a running command is busy");
    queue.cancel_active();
    wait_until("the queue to go idle", || queue.active_command().is_none()).await;
    assert_eq!(
        hub.current(),
        gateway_api_types::Progress::default(),
        "the body's return drops the activity and the hub falls idle"
    );
    queue.shutdown();
}

#[tokio::test]
async fn a_pre_cancelled_load_profile_settles_as_cancelled_without_switching() {
    let state = state();
    let token = CancellationToken::new();
    token.cancel();
    let activity = state.hub.begin("test");
    let outcome = run_command(
        state.clone(),
        Command::load_profile(ProfileName::parse("alpha").expect("profile name"), token),
        activity,
    )
    .await;
    assert!(
        matches!(outcome, Err(GatewayError::CommandCancelled(_))),
        "a fired token stops the load before any phase: {outcome:?}"
    );
    assert!(
        state.live.read().await.loading.is_empty(),
        "the cancelled load never touched the live state"
    );
}

#[tokio::test]
async fn an_enqueue_on_a_closed_queue_settles_immediately() {
    let queue = queue();
    queue.shutdown();
    let handle = queue.enqueue(load_profile("alpha"));
    let outcome = handle.outcome.await.expect("settled at enqueue");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    assert!(queue.pending_commands().is_empty());
}

/// An executor that records each command's label in run order, then
/// settles it Ok.
fn recording_executor(order: &Arc<Mutex<Vec<String>>>) -> Arc<Executor> {
    Arc::new({
        let order = Arc::clone(order);
        move |_state, command: Command, _activity| {
            let order = Arc::clone(&order);
            Box::pin(async move {
                let label = command.label();
                order
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(label.clone());
                Ok(label)
            }) as BoxFuture<'static, Outcome>
        }
    })
}

#[tokio::test]
async fn the_worker_drains_more_than_thirty_two_commands_in_fifo_order() {
    let state = state();
    let queue = state.commands.clone();
    let order = Arc::new(Mutex::new(Vec::new()));
    // The pending deque is unbounded: every command waits for the one
    // worker, well past the old channel's capacity.
    let handles: Vec<Enqueued> = (0..40)
        .map(|index| queue.enqueue(unload(&format!("m{index}"))))
        .collect();
    let worker = state
        .commands
        .spawn_worker_with(&state, recording_executor(&order))
        .expect("worker spawns");

    for handle in handles {
        let outcome = handle.outcome.await.expect("each command settles");
        assert!(outcome.is_ok(), "no command is rejected: {outcome:?}");
    }
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");

    let expected: Vec<String> = (0..40)
        .map(|index| format!("unload-model: m{index}"))
        .collect();
    assert_eq!(
        order
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        expected.as_slice(),
        "the deque holds every command and the worker drains them FIFO"
    );
}

#[tokio::test]
async fn the_queue_spawns_at_most_one_worker() {
    let state = state();
    let queue = state.commands.clone();
    let first = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("the first worker spawns");
    assert!(
        state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .is_none(),
        "a second worker is refused"
    );
    queue.shutdown();
    first.await.expect("the worker exits on shutdown");
}

#[tokio::test]
async fn shutdown_on_an_idle_queue_stops_the_parked_worker() {
    let state = state();
    let queue = state.commands.clone();
    let worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    // Let the worker park on the empty deque.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    queue.shutdown();
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("the parked worker wakes and exits")
        .expect("the worker task joins");
}

#[tokio::test]
async fn an_enqueue_wakes_the_parked_worker() {
    let state = state();
    let queue = state.commands.clone();
    let order = Arc::new(Mutex::new(Vec::new()));
    let worker = state
        .commands
        .spawn_worker_with(&state, recording_executor(&order))
        .expect("worker spawns");
    // Let the worker park before the command lands.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let handle = queue.enqueue(unload("m"));
    let outcome = handle.outcome.await.expect("the command settles");
    assert!(
        outcome.is_ok(),
        "the parked worker woke and ran it: {outcome:?}"
    );
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

#[tokio::test]
async fn an_enqueue_racing_shutdown_around_the_workers_sleep_still_settles() {
    let state = state();
    let queue = state.commands.clone();
    let worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let handle = queue.enqueue(load_profile("alpha"));
    queue.shutdown();
    let outcome = handle.outcome.await.expect("the command settles");
    assert!(
        matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
        "whether drained or started, shutdown settles it as cancelled: {outcome:?}"
    );
    worker.await.expect("the worker exits on shutdown");
}

#[tokio::test]
async fn a_cancelled_pending_command_never_reaches_the_worker() {
    let state = state();
    let queue = state.commands.clone();
    let order = Arc::new(Mutex::new(Vec::new()));
    let cancelled = queue.enqueue(unload("a"));
    let kept = queue.enqueue(unload("b"));
    assert!(queue.cancel_pending(0), "the first entry leaves the deque");
    let worker = state
        .commands
        .spawn_worker_with(&state, recording_executor(&order))
        .expect("worker spawns");

    let outcome = cancelled
        .outcome
        .await
        .expect("the cancelled command settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    let outcome = kept.outcome.await.expect("the kept command settles");
    assert!(outcome.is_ok(), "the kept command runs: {outcome:?}");
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");
    assert_eq!(
        order
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        ["unload-model: b"],
        "the worker never saw the cancelled entry"
    );
}

#[tokio::test]
async fn an_unload_of_a_model_the_runtime_does_not_hold_is_unknown_model() {
    let state = state();
    let activity = state.hub.begin("test");
    let outcome = run_command(state.clone(), unload("ghost"), activity).await;
    assert!(
        matches!(&outcome, Err(GatewayError::UnknownModel(name)) if name == "ghost"),
        "an unload miss is UnknownModel, not a queue error: {outcome:?}"
    );
}

/// A two-profile remote catalog on an endpoint nothing listens on:
/// remote routing is static, so the boot load succeeds without network.
#[cfg(feature = "stt")]
fn speech_state() -> AppState {
    let catalog = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"alpha-model\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"a\"\nendpoints = [\"e\"]\n\
         [[model]]\nname = \"beta-model\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"b\"\nendpoints = [\"e\"]\n\
         [[profile]]\nname = \"alpha\"\nmodels = []\n\
         [[profile]]\nname = \"beta\"\nmodels = []\n",
    )
    .expect("catalog parses");
    crate::test_support::boot_state(catalog)
}

/// The boot load runs its local half first, then makes the process's
/// one guarded STT load attempt.
#[cfg(feature = "stt")]
#[tokio::test]
async fn the_boot_command_loads_speech_after_its_local_half() {
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

    let mut state = speech_state();
    crate::test_support::arm_boot_speech(
        &mut state,
        ScriptedModelFactory::new(ScriptedDecoder::new()),
    );
    let activity = state.hub.begin("test");
    let outcome = run_command(state.clone(), load_profile("alpha"), activity).await;

    assert_eq!(
        outcome.as_deref().ok(),
        Some("alpha"),
        "the boot command settles with the profile: {outcome:?}"
    );
    assert!(
        state.live.read().await.routing.model("alpha-model").is_ok(),
        "the remote table the runner published keeps serving"
    );
    assert!(
        state.speech.status().ready(),
        "the boot command's guarded load published the speech runtime"
    );
    assert_eq!(
        state
            .speech
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim"]
    );
    state.speech.shutdown();
}

/// A duplicate attaching to the pending boot command shares its outcome;
/// the single load attempt runs once for both waiters.
#[cfg(feature = "stt")]
#[tokio::test]
async fn a_duplicate_attached_to_the_boot_command_shares_the_single_speech_load() {
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

    let mut state = speech_state();
    crate::test_support::arm_boot_speech(
        &mut state,
        ScriptedModelFactory::new(ScriptedDecoder::new()),
    );
    let queue = state.commands.clone();
    // Both enqueue before the worker spawns, so the attach cannot race
    // the drain.
    let boot_handle = queue.enqueue(load_profile("alpha"));
    let attached = queue.enqueue(load_profile("alpha"));
    assert_eq!(
        boot_handle.entry, attached.entry,
        "the duplicate attaches to the boot command"
    );
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");

    let outcome = boot_handle.outcome.await.expect("the boot command settles");
    assert!(outcome.is_ok(), "the boot command succeeds: {outcome:?}");
    let outcome = attached.outcome.await.expect("the attached waiter settles");
    assert!(
        outcome.is_ok(),
        "the attached duplicate shares the outcome: {outcome:?}"
    );
    assert!(
        state.speech.status().ready(),
        "the one guarded load published the runtime for both waiters"
    );
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");
    state.speech.shutdown();
}
