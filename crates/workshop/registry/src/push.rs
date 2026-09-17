//! The intent-named push facade over the producer sink slots: business
//! code reports what happened and never chooses a severity or builds a
//! bus payload (SiYuan's `PushReloadFiletree` pattern).
//!
//! Producers hold a [`Push`] and speak in intents - a status update, a
//! failure, an activity pulse, determinate progress, idle, a fresh model
//! catalog; workbench producers drive the Model-menu mutators through
//! [`Push::menu`], and every mutation publishes its own snapshot. What
//! each intent becomes on the wire is decided here and in
//! `workshop-protocol`, nowhere else. The sinks stay the transport:
//! every `/ws` session subscribes to the status, catalog, and menu
//! buses behind the sink slots and serializes what it receives.
//!
//! Every intent degrades to a no-op while its sink slot is empty, so a
//! producer spawned before its subsystem registers never fails.

use workshop_protocol::{Activity, Progress, Severity, StatusBarUpdate};

use crate::Registry;
use crate::traits::{CatalogSink, MenuSink, StatusSink};

/// The intent-named push handle over the status, catalog, and menu sink
/// slots.
///
/// Clones are cheap (a few `Arc` bumps) and every clone reads the same
/// registry slots, so producers take their own copy, exactly as they did
/// with the buses themselves.
#[derive(Debug, Clone)]
pub struct Push {
    registry: Registry,
}

impl Push {
    /// Wraps the registry whose sink slots every unsolicited push flows
    /// through.
    pub(crate) fn new(registry: Registry) -> Self {
        Self { registry }
    }

    /// Pushes a user-visible status update: a `{"type":"status",...}`
    /// `StatusFrame` at info severity with no progress.
    pub fn push_status_update(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.emit(label, description, None, Severity::Info, activity);
    }

    /// Pushes a failure the user should see: a `{"type":"status",...}`
    /// `StatusFrame` at error severity.
    pub fn push_failure(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.emit(label, description, None, Severity::Error, activity);
    }

    /// Pushes an activity pulse the UI does not display as text: a
    /// `{"type":"status",...}` `StatusFrame` at debug severity, whose
    /// `activity` field drives the status bar's LED.
    pub fn push_activity(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.emit(label, description, None, Severity::Debug, activity);
    }

    /// Pushes determinate progress - `current` of `total` units done: a
    /// `{"type":"status",...}` `StatusFrame` at [`Severity::Info`]
    /// carrying a [`Progress`], which the status bar renders as its
    /// progress bar.
    pub fn push_progress(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        current: u64,
        total: u64,
        activity: Activity,
    ) {
        self.emit(
            label,
            description,
            Some(Progress { current, total }),
            Severity::Info,
            activity,
        );
    }

    /// Pushes the status bar back to its resting state: the
    /// `Ready`/`idle` `{"type":"status",...}` `StatusFrame`.
    pub fn push_idle(&self) {
        self.push_status_update("Ready", "idle", Activity::General);
    }

    /// Pushes one complete model catalog snapshot: a
    /// `{"type":"models",...}` `CatalogFrame` carrying only chat-capable
    /// entries. The single choke point for catalog publishes: the menu
    /// revalidates its selection against the new catalog and republishes
    /// the workbench snapshot when it changed.
    pub fn push_models_catalog(&self, models: Vec<serde_json::Value>) {
        if let Some(catalog) = self.registry.sink::<dyn CatalogSink>() {
            catalog.publish(models);
        }
        if let Some(menu) = self.registry.sink::<dyn MenuSink>() {
            menu.reconcile_catalog();
        }
    }

    /// The menu sink behind the facade, for producers that drive the
    /// Model-menu mutators directly: the heartbeat feeds reachability
    /// and the gateway's profile state through this handle.
    #[must_use]
    pub fn menu(&self) -> MenuPush {
        MenuPush {
            registry: self.registry.clone(),
        }
    }

    /// Builds one status frame and emits it into the status sink; a
    /// no-op while the status subsystem has not registered.
    fn emit(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        progress: Option<Progress>,
        severity: Severity,
        activity: Activity,
    ) {
        let Some(status) = self.registry.sink::<dyn StatusSink>() else {
            return;
        };
        status.emit(StatusBarUpdate {
            label: label.into(),
            description: description.into(),
            progress,
            severity,
            activity,
        });
    }
}

/// The menu-mutator face of [`Push`]: the workbench mutators the
/// gateway subsystem drives, reading the registry's menu sink slot.
/// Every mutator is a no-op while the menu subsystem has not
/// registered.
#[derive(Debug, Clone)]
pub struct MenuPush {
    registry: Registry,
}

impl MenuPush {
    /// Records the heartbeat's verdict on the gateway; `chat_ready` is
    /// false while the gateway is down.
    pub fn set_gateway_reachable(&self, reachable: bool) {
        if let Some(menu) = self.registry.sink::<dyn MenuSink>() {
            menu.set_gateway_reachable(reachable);
        }
    }

    /// Records the gateway's profile list and active profile. A gateway
    /// without profile support feeds an empty list - a state, not an
    /// error.
    pub fn set_profiles(&self, profiles: Vec<String>, active: Option<String>) {
        if let Some(menu) = self.registry.sink::<dyn MenuSink>() {
            menu.set_profiles(profiles, active);
        }
    }

    /// Restores a boot-time selection when none is applied; with a
    /// selection already applied this is a no-op.
    pub fn restore_selection(&self) {
        if let Some(menu) = self.registry.sink::<dyn MenuSink>() {
            menu.restore_selection();
        }
    }
}

#[cfg(test)]
#[path = "push-tests.rs"]
mod tests;
