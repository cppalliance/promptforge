//! Tests for the [`Push`] facade: every intent lands on the right sink
//! with the right frame, and an empty slot degrades the intent to a
//! no-op. The sinks are recording adapters - closures capturing into
//! channels - so the assertions read exactly like the bus-receiver
//! assertions the producers' own crates carry.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use super::*;
use crate::{
    CatalogSinkAdapter, MenuSinkAdapter, Registration, StatusSink, StatusSinkAdapter,
    traits::{CatalogSink, MenuSink},
};
use workshop_protocol::StatusBarUpdate;

/// A registry wired with recording sinks: status and catalog frames go
/// to broadcast receivers, menu mutator calls to a shared log.
struct Recording {
    push: Push,
    status_rx: broadcast::Receiver<StatusBarUpdate>,
    catalog_rx: broadcast::Receiver<Vec<serde_json::Value>>,
    menu_calls: Arc<Mutex<Vec<String>>>,
    // The registrations keep the sinks alive for the test's duration.
    _guards: (Registration, Registration, Registration),
}

/// Wires a registry with recording sink adapters and returns its push
/// facade plus the recording ends.
fn wired() -> Recording {
    let registry = Registry::new();
    let (status_tx, status_rx) = broadcast::channel(16);
    let (catalog_tx, catalog_rx) = broadcast::channel(16);
    let menu_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let status_guard =
        registry.register_sink::<dyn StatusSink>(Arc::new(StatusSinkAdapter::new(move |update| {
            // A send only fails when the test dropped its receiver.
            let _ = status_tx.send(update);
        })));
    let catalog_guard = registry.register_sink::<dyn CatalogSink>(Arc::new(
        CatalogSinkAdapter::new(move |models| {
            let _ = catalog_tx.send(models);
        }),
    ));
    let menu_guard = {
        let calls = Arc::clone(&menu_calls);
        registry.register_sink::<dyn MenuSink>(Arc::new(MenuSinkAdapter::new(
            {
                let calls = Arc::clone(&calls);
                move |reachable| record(&calls, format!("reachable:{reachable}"))
            },
            {
                let calls = Arc::clone(&calls);
                move |profiles, active| {
                    record(&calls, format!("profiles:{}:{active:?}", profiles.len()));
                }
            },
            {
                let calls = Arc::clone(&calls);
                move || record(&calls, "restore".to_string())
            },
            move || record(&calls, "reconcile".to_string()),
        )))
    };
    Recording {
        push: registry.push(),
        status_rx,
        catalog_rx,
        menu_calls,
        _guards: (status_guard, catalog_guard, menu_guard),
    }
}

/// Appends one menu mutator call to the recording.
fn record(calls: &Mutex<Vec<String>>, call: String) {
    calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(call);
}

/// The menu mutator calls recorded so far.
fn menu_calls(recording: &Recording) -> Vec<String> {
    recording
        .menu_calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn a_status_update_reaches_the_sink_at_info_severity() {
    let mut recording = wired();
    recording.push.push_status_update(
        "Connected to gateway",
        "the probe answered",
        Activity::General,
    );
    let update = recording
        .status_rx
        .recv()
        .await
        .expect("the update reaches the sink");
    assert_eq!(
        update,
        StatusBarUpdate {
            label: "Connected to gateway".to_string(),
            description: "the probe answered".to_string(),
            progress: None,
            severity: Severity::Info,
            activity: Activity::General,
        }
    );
}

#[tokio::test]
async fn a_failure_reaches_the_sink_at_error_severity() {
    let mut recording = wired();
    recording
        .push
        .push_failure("Connection lost", "the gateway hung up", Activity::General);
    let update = recording
        .status_rx
        .recv()
        .await
        .expect("the update reaches the sink");
    assert_eq!(
        update,
        StatusBarUpdate {
            label: "Connection lost".to_string(),
            description: "the gateway hung up".to_string(),
            progress: None,
            severity: Severity::Error,
            activity: Activity::General,
        }
    );
}

#[tokio::test]
async fn an_activity_pulse_reaches_the_sink_at_debug_severity() {
    let mut recording = wired();
    recording.push.push_activity(
        "Streaming response...",
        "a gateway response chunk",
        Activity::Generating,
    );
    let update = recording
        .status_rx
        .recv()
        .await
        .expect("the update reaches the sink");
    assert_eq!(
        update,
        StatusBarUpdate {
            label: "Streaming response...".to_string(),
            description: "a gateway response chunk".to_string(),
            progress: None,
            severity: Severity::Debug,
            activity: Activity::Generating,
        }
    );
}

#[tokio::test]
async fn progress_reaches_the_sink_with_its_current_and_total_counts() {
    let mut recording = wired();
    recording.push.push_progress(
        "Downloading model",
        "ggml-large-v3.bin",
        5,
        12,
        Activity::General,
    );
    let update = recording
        .status_rx
        .recv()
        .await
        .expect("the update reaches the sink");
    assert_eq!(
        update,
        StatusBarUpdate {
            label: "Downloading model".to_string(),
            description: "ggml-large-v3.bin".to_string(),
            progress: Some(Progress {
                current: 5,
                total: 12,
            }),
            severity: Severity::Info,
            activity: Activity::General,
        }
    );
}

#[tokio::test]
async fn idle_reaches_the_sink_as_the_resting_update() {
    let mut recording = wired();
    recording.push.push_idle();
    let update = recording
        .status_rx
        .recv()
        .await
        .expect("the update reaches the sink");
    assert_eq!(
        update,
        StatusBarUpdate {
            label: "Ready".to_string(),
            description: "idle".to_string(),
            progress: None,
            severity: Severity::Info,
            activity: Activity::General,
        }
    );
}

#[tokio::test]
async fn a_models_catalog_reaches_the_sink_as_one_snapshot() {
    let mut recording = wired();
    let models = vec![serde_json::json!({"id": "test-model", "object": "model"})];
    recording.push.push_models_catalog(models.clone());
    let received = recording
        .catalog_rx
        .recv()
        .await
        .expect("the push reaches the sink");
    assert_eq!(received, models, "the catalog sink takes the raw models");
}

#[tokio::test]
async fn a_catalog_push_reconciles_the_workbench_selection() {
    let recording = wired();
    recording
        .push
        .push_models_catalog(vec![serde_json::json!({"id": "model-a"})]);
    assert_eq!(
        menu_calls(&recording),
        ["reconcile"],
        "every catalog publish revalidates the menu's selection"
    );
}

#[test]
fn the_menu_handle_drives_the_menu_sink_mutators() {
    let recording = wired();
    let menu = recording.push.menu();
    menu.set_gateway_reachable(true);
    menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
    menu.restore_selection();
    assert_eq!(
        menu_calls(&recording),
        ["reachable:true", "profiles:1:Some(\"main\")", "restore"],
    );
}

#[test]
fn intents_on_empty_slots_are_no_ops() {
    let push = Registry::new().push();
    // None of these panic or fail without registered sinks.
    push.push_status_update("label", "description", Activity::General);
    push.push_failure("label", "description", Activity::General);
    push.push_activity("label", "description", Activity::General);
    push.push_progress("label", "description", 1, 2, Activity::General);
    push.push_idle();
    push.push_models_catalog(Vec::new());
    let menu = push.menu();
    menu.set_gateway_reachable(false);
    menu.set_profiles(Vec::new(), None);
    menu.restore_selection();
}

#[test]
fn dropping_a_registration_stops_its_intents() {
    let registry = Registry::new();
    let (status_tx, mut status_rx) = broadcast::channel(16);
    let guard =
        registry.register_sink::<dyn StatusSink>(Arc::new(StatusSinkAdapter::new(move |update| {
            let _ = status_tx.send(update);
        })));
    let push = registry.push();
    push.push_idle();
    assert!(
        status_rx.try_recv().is_ok(),
        "the registered sink receives the intent"
    );
    drop(guard);
    push.push_idle();
    assert!(
        status_rx.try_recv().is_err(),
        "the dropped registration deregisters the sink"
    );
}
