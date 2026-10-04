//! Tests for the command queue's debouncing, ordering, and cancellation.

use std::time::Duration;

use gateway_config::Config;

use super::*;
use crate::test_support::{app_state, parking_executor, wait_until};

#[path = "tests-worker.rs"]
mod worker;

fn queue() -> CommandQueue {
    CommandQueue::new(Arc::new(ProgressHub::new()))
}

fn load_profile(name: &str) -> Command {
    Command::load_profile(
        ProfileName::parse(name).expect("profile name"),
        CancellationToken::new(),
    )
}

fn provision(name: &str) -> Command {
    Command::ProvisionModel {
        name: name.to_owned(),
        source: format!("/models/{name}.gguf"),
        token: CancellationToken::new(),
    }
}

fn unload(name: &str) -> Command {
    Command::UnloadModel {
        name: name.to_owned(),
    }
}

/// An `ApplyConfig` over a one-profile config with nothing captured; the
/// stub executors never read the snapshot.
fn apply() -> Command {
    let config = Config::from_toml_str(
        "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"t\"\n\
         [[profile]]\nname = \"alpha\"\nmodels = []\n",
    )
    .expect("config parses");
    Command::ApplyConfig {
        snapshot: ApplySnapshot {
            config: Box::new(config),
            files: Vec::new(),
            restart_required: false,
        },
        token: CancellationToken::new(),
    }
}

/// An `AppState` over a minimal config; the stub executors never read it.
fn state() -> AppState {
    let config = Config::from_toml_str(
        "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"t\"\n",
    )
    .expect("config parses");
    app_state(config, None)
}

/// Whether the active command is the one labelled `name`.
fn active_is(queue: &CommandQueue, name: &str) -> bool {
    queue
        .active_command()
        .is_some_and(|status| status.name == name)
}

fn pending_names(queue: &CommandQueue) -> Vec<String> {
    queue
        .pending_commands()
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

#[test]
fn a_duplicate_load_profile_for_the_same_profile_is_dropped() {
    let queue = queue();
    let first = queue.enqueue(load_profile("alpha"));
    let second = queue.enqueue(load_profile("alpha"));

    let pending = queue.pending_commands();
    assert_eq!(pending.len(), 1, "the duplicate never enters the queue");
    assert_eq!(
        first.entry, second.entry,
        "the duplicate attaches to the pending command's entry"
    );
    assert!(queue.active_command().is_none());
}

#[test]
fn an_apply_attaches_to_a_pending_apply() {
    let queue = queue();
    let first = queue.enqueue(apply());
    let second = queue.enqueue(apply());

    assert_eq!(
        pending_names(&queue),
        ["apply-config"],
        "one apply is pending; the duplicate never enters the queue"
    );
    assert_eq!(
        first.entry, second.entry,
        "the duplicate attaches to the pending apply's entry"
    );
}

#[test]
fn a_load_profile_queues_behind_a_pending_apply_without_replacing_it() {
    let queue = queue();
    let applied = queue.enqueue(apply());
    let _switch = queue.enqueue(load_profile("alpha"));

    assert_eq!(
        pending_names(&queue),
        ["apply-config", "load-profile: alpha"],
        "the switch queues FIFO behind the apply"
    );
    drop(applied);
}

/// An apply enqueued while the boot load is active queues behind it
/// FIFO: the boot load keeps running, its waiter is not settled, and
/// the apply starts only once the boot load has settled.
#[tokio::test]
async fn an_apply_during_the_active_boot_load_queues_behind_it_without_cancelling_it() {
    let state = state();
    let queue = state.commands.clone();
    let _worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");

    let mut boot = queue.enqueue(load_profile("alpha"));
    wait_until("the boot load to go active", || {
        active_is(&queue, "load-profile: alpha")
    })
    .await;
    let applied = queue.enqueue(apply());
    // Give the worker every chance to act on a cancellation that must
    // not have happened.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert!(
        active_is(&queue, "load-profile: alpha"),
        "the boot load keeps running under the queued apply"
    );
    assert_eq!(pending_names(&queue), ["apply-config"]);
    assert!(
        matches!(
            boot.outcome.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "the boot load's waiter is not settled by the apply"
    );

    assert!(queue.cancel_active(), "the boot load is cancelled by hand");
    let outcome = boot.outcome.await.expect("the boot load settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    wait_until("the apply to go active", || {
        active_is(&queue, "apply-config")
    })
    .await;
    assert!(queue.cancel_active());
    let outcome = applied.outcome.await.expect("the apply settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    queue.shutdown();
}

#[tokio::test]
async fn a_load_profile_queues_behind_an_active_apply_without_cancelling_it() {
    let state = state();
    let queue = state.commands.clone();
    let _worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");

    let mut applied = queue.enqueue(apply());
    wait_until("the apply to go active", || {
        active_is(&queue, "apply-config")
    })
    .await;
    let switch = queue.enqueue(load_profile("alpha"));
    // Give the worker every chance to act on a cancellation that must
    // not have happened.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert!(
        active_is(&queue, "apply-config"),
        "the apply keeps running under the queued switch"
    );
    assert_eq!(pending_names(&queue), ["load-profile: alpha"]);
    assert!(
        matches!(
            applied.outcome.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "the apply's waiter is not settled by the switch"
    );

    assert!(queue.cancel_active());
    let outcome = applied.outcome.await.expect("the apply settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    wait_until("alpha to go active", || {
        active_is(&queue, "load-profile: alpha")
    })
    .await;
    assert!(queue.cancel_active());
    let outcome = switch.outcome.await.expect("alpha settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    queue.shutdown();
}

#[tokio::test]
async fn cancel_apply_removes_a_pending_apply_and_fires_an_active_one() {
    // Pending: no worker, so the apply waits in the queue.
    let queue = queue();
    assert!(
        !queue.cancel_apply(),
        "an idle queue has no apply to cancel"
    );
    let _boot = queue.enqueue(load_profile("alpha"));
    assert!(
        !queue.cancel_apply(),
        "a pending boot load is not an apply and stays put"
    );
    assert_eq!(pending_names(&queue), ["load-profile: alpha"]);
    let pending = queue.enqueue(apply());
    assert!(queue.cancel_apply(), "the pending apply is removed");
    assert_eq!(
        pending_names(&queue),
        ["load-profile: alpha"],
        "only the apply leaves the queue"
    );
    let outcome = pending.outcome.await.expect("the removed apply settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));

    // Active: the parked apply observes its token.
    let state = state();
    let queue = state.commands.clone();
    let _worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    let active = queue.enqueue(apply());
    wait_until("the apply to go active", || {
        active_is(&queue, "apply-config")
    })
    .await;
    assert!(queue.cancel_apply(), "the active apply's token fires");
    let outcome = active.outcome.await.expect("the active apply settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    wait_until("the queue to go idle", || queue.active_command().is_none()).await;
    queue.shutdown();
}

#[test]
fn provision_model_debounces_on_the_model_name() {
    let queue = queue();
    let first = queue.enqueue(provision("m"));
    let duplicate = queue.enqueue(provision("m"));
    let _other = queue.enqueue(provision("n"));

    assert_eq!(
        first.entry, duplicate.entry,
        "a same-model duplicate attaches to the pending command"
    );
    let pending = queue.pending_commands();
    assert_eq!(pending.len(), 2, "distinct models queue independently");
    assert!(
        pending
            .iter()
            .any(|entry| entry.name == "provision-model: m")
    );
    assert!(
        pending
            .iter()
            .any(|entry| entry.name == "provision-model: n")
    );
}

#[test]
fn unload_model_is_never_debounced() {
    let queue = queue();
    let first = queue.enqueue(unload("m"));
    let second = queue.enqueue(unload("m"));

    assert_eq!(queue.pending_commands().len(), 2);
    assert_ne!(first.entry, second.entry, "each unload keeps its own entry");
}

#[tokio::test]
async fn cancel_pending_removes_the_entry_and_settles_its_waiter() {
    let queue = queue();
    let _switch = queue.enqueue(load_profile("alpha"));
    let provisioned = queue.enqueue(provision("m"));

    assert!(!queue.cancel_pending(5), "out of range is a no-op");
    assert!(queue.cancel_pending(1), "the provision entry is removed");
    let pending = queue.pending_commands();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "load-profile: alpha");
    let outcome = provisioned.outcome.await.expect("the waiter settles");
    assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
}
