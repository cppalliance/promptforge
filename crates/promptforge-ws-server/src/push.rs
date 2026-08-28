//! The push facade: intent-named send methods over the status, catalog,
//! and menu broadcast buses, so business code reports what happened and
//! never chooses a severity or builds a bus payload (SiYuan's
//! `PushReloadFiletree` pattern).
//!
//! Producers hold a [`Push`] and speak in intents - a status update, a
//! failure, an activity pulse, determinate progress, idle, a fresh model
//! catalog; workbench producers drive the Model-menu mutators through
//! [`Push::menu`], and every mutation publishes its own snapshot. What
//! each intent becomes on the wire
//! is decided here and in [`crate::protocol`], nowhere else. The buses
//! stay the transport: every `/ws` session subscribes on
//! [`crate::status::StatusBus`], [`crate::catalog::CatalogBus`], and
//! [`crate::menu::MenuBus`] and serializes what it receives.

use crate::catalog::CatalogBus;
use crate::menu::MenuBus;
use crate::protocol::{Activity, Progress};
use crate::status::StatusBus;

/// The intent-named push handle over the status, catalog, and menu buses.
///
/// Clones are cheap (a few `Arc` bumps) and every clone feeds the same
/// buses, so producers take their own copy, exactly as they did with the
/// buses themselves.
#[derive(Debug, Clone)]
pub(crate) struct Push {
    status: StatusBus,
    catalog: CatalogBus,
    menu: MenuBus,
}

impl Push {
    /// Wraps the buses every unsolicited push flows through.
    pub(crate) fn new(status: StatusBus, catalog: CatalogBus, menu: MenuBus) -> Self {
        Self {
            status,
            catalog,
            menu,
        }
    }

    /// Pushes a user-visible status update: a `{"type":"status",...}`
    /// [`crate::protocol::StatusFrame`] at
    /// [`Severity::Info`](crate::protocol::Severity::Info) with no
    /// progress.
    pub(crate) fn push_status_update(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.status.info(label, description, activity);
    }

    /// Pushes a failure the user should see: a `{"type":"status",...}`
    /// [`crate::protocol::StatusFrame`] at
    /// [`Severity::Error`](crate::protocol::Severity::Error).
    pub(crate) fn push_failure(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.status.error(label, description, activity);
    }

    /// Pushes an activity pulse the UI does not display as text: a
    /// `{"type":"status",...}` [`crate::protocol::StatusFrame`] at
    /// [`Severity::Debug`](crate::protocol::Severity::Debug), whose
    /// `activity` field drives the status bar's LED.
    pub(crate) fn push_activity(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.status.debug(label, description, activity);
    }

    /// Pushes determinate progress - `current` of `total` units done: a
    /// `{"type":"status",...}` [`crate::protocol::StatusFrame`] at
    /// [`Severity::Info`](crate::protocol::Severity::Info) carrying a
    /// [`Progress`], which the status bar renders as its progress bar.
    pub(crate) fn push_progress(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        current: u64,
        total: u64,
        activity: Activity,
    ) {
        self.status
            .progress(label, description, Progress { current, total }, activity);
    }

    /// Pushes the status bar back to its resting state: the `Ready`/`idle`
    /// `{"type":"status",...}` [`crate::protocol::StatusFrame`].
    pub(crate) fn push_idle(&self) {
        self.status.idle();
    }

    /// Pushes one complete model catalog snapshot: a `{"type":"models",...}`
    /// [`crate::protocol::CatalogFrame`] carrying the gateway's `data`
    /// array verbatim. The single choke point for catalog publishes: the
    /// menu revalidates its selection against the new catalog and
    /// republishes the workbench snapshot when it changed.
    pub(crate) fn push_models_catalog(&self, models: Vec<serde_json::Value>) {
        self.catalog.publish(models);
        self.menu.reconcile_catalog();
    }

    /// The menu bus behind the facade, for producers that drive the
    /// Model-menu mutators directly: the heartbeat feeds reachability
    /// and the gateway's profile state through this handle.
    pub(crate) fn menu(&self) -> &MenuBus {
        &self.menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::sync::broadcast;

    use crate::protocol::{CatalogPush, Severity, StatusBarUpdate};

    /// A push handle plus one receiver on the status and catalog buses.
    fn wired() -> (
        Push,
        broadcast::Receiver<StatusBarUpdate>,
        broadcast::Receiver<CatalogPush>,
    ) {
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        let status_rx = status.subscribe();
        let catalog_rx = catalog.subscribe();
        let menu = MenuBus::new(catalog.clone(), None);
        (Push::new(status, catalog, menu), status_rx, catalog_rx)
    }

    /// A push handle plus the menu bus it feeds, for the workbench tests.
    fn wired_with_menu() -> (Push, MenuBus) {
        let catalog = CatalogBus::new();
        let menu = MenuBus::new(catalog.clone(), None);
        (Push::new(StatusBus::new(), catalog, menu.clone()), menu)
    }

    #[tokio::test]
    async fn a_status_update_reaches_the_bus_at_info_severity() {
        let (push, mut rx, _catalog_rx) = wired();
        push.push_status_update(
            "Connected to gateway",
            "the probe answered",
            Activity::General,
        );
        let update = rx.recv().await.expect("the update reaches the bus");
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
    async fn a_failure_reaches_the_bus_at_error_severity() {
        let (push, mut rx, _catalog_rx) = wired();
        push.push_failure("Connection lost", "the gateway hung up", Activity::General);
        let update = rx.recv().await.expect("the update reaches the bus");
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
    async fn an_activity_pulse_reaches_the_bus_at_debug_severity() {
        let (push, mut rx, _catalog_rx) = wired();
        push.push_activity(
            "Streaming response...",
            "a gateway response chunk",
            Activity::Generating,
        );
        let update = rx.recv().await.expect("the update reaches the bus");
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
    async fn progress_reaches_the_bus_with_its_current_and_total_counts() {
        let (push, mut rx, _catalog_rx) = wired();
        push.push_progress(
            "Downloading model",
            "ggml-large-v3.bin",
            5,
            12,
            Activity::General,
        );
        let update = rx.recv().await.expect("the update reaches the bus");
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
    async fn idle_reaches_the_bus_as_the_resting_update() {
        let (push, mut rx, _catalog_rx) = wired();
        push.push_idle();
        let update = rx.recv().await.expect("the update reaches the bus");
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
    async fn a_models_catalog_reaches_the_bus_as_one_snapshot() {
        let (push, _status_rx, mut rx) = wired();
        let models = vec![serde_json::json!({"id": "test-model", "object": "model"})];
        push.push_models_catalog(models.clone());
        let received = rx.recv().await.expect("the push reaches the bus");
        assert_eq!(received, CatalogPush { models });
    }

    #[test]
    fn a_catalog_push_reconciles_the_workbench_selection() {
        let (push, menu) = wired_with_menu();
        push.push_models_catalog(vec![serde_json::json!({"id": "model-a"})]);
        menu.set_selected("model-a")
            .expect("the id is in the catalog");
        push.push_models_catalog(vec![serde_json::json!({"id": "model-b"})]);
        let snapshot = menu.latest().expect("the reconcile republished");
        assert_eq!(
            snapshot.selected_model, None,
            "the selection the new catalog no longer holds is revalidated away"
        );
    }
}
