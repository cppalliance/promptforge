//! The server-owned Model menu: the workbench snapshot pushed to every
//! `/ws` session as a `{"type":"workbench",...}` frame, its broadcast
//! bus, and the per-profile model memory persisted beside the tape file.
//!
//! The server owns all Model-menu state and the UI only renders it; in
//! particular `chat_ready` is computed here - catalog non-empty, a model
//! selected, no switch in flight, gateway reachable - and never derived
//! client-side. Like the catalog bus, the channel is a tokio broadcast:
//! publishing never blocks, a publish with no sessions is a no-op, and a
//! lagging session skips ahead - every push is a complete snapshot, so an
//! overwritten one loses nothing. The bus also retains the newest push,
//! so a session that connects later sends the current menu immediately -
//! the delivery contract's resend-on-reconnect for ephemeral frames.
//!
//! Mutation is zone two throughout: a refused mutation (an unknown model
//! id, a second switch while one runs) is a value returned to the caller,
//! and a missing, unreadable, or corrupt memory file means "no memory
//! yet" - logged and tolerated, never fatal. The memory file holds server
//! state only; the UI's panel layout is view state and stays in the
//! webview's localStorage.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use crate::catalog::CatalogBus;
use crate::protocol::WorkbenchSnapshot;

/// Ring capacity of the menu bus. Pushes follow user interactions and
/// heartbeat transitions, so a handful of slots is generous.
const MENU_CHANNEL_CAPACITY: usize = 8;

/// Name of the persisted server-state file, written in the directory
/// holding the tape file.
const WORKSHOP_STATE_FILE: &str = "workshop-state.json";

/// The shared menu bus: the Model-menu state, its mutators, and the
/// broadcast channel their snapshots fan out on, mirroring
/// [`crate::catalog::CatalogBus`].
///
/// Clones are cheap (a few `Arc` bumps) and share one state, one retained
/// snapshot, and one channel.
#[derive(Debug, Clone)]
pub(crate) struct MenuBus {
    sender: broadcast::Sender<WorkbenchSnapshot>,
    latest: Arc<Mutex<Option<WorkbenchSnapshot>>>,
    state: Arc<Mutex<MenuState>>,
    // Selections are validated against the retained catalog and
    // `chat_ready` reads its emptiness, so the menu holds its own handle.
    catalog: CatalogBus,
}

/// The mutable Model-menu state behind the bus.
#[derive(Debug)]
struct MenuState {
    /// Every gateway profile name, in gateway order.
    profiles: Vec<String>,
    /// The profile the gateway is serving, once known.
    active: Option<String>,
    /// The profile a switch is loading, while one is in flight.
    switching: Option<String>,
    /// The model chat requests go to, once one is selected.
    selected_model: Option<String>,
    /// The heartbeat's verdict on the gateway.
    gateway_reachable: bool,
    /// Remembered model selection per profile name, persisted to
    /// [`WORKSHOP_STATE_FILE`].
    last_selected: HashMap<String, String>,
    /// Where the memory persists; `None` disables persistence.
    memory_path: Option<PathBuf>,
}

impl MenuState {
    /// Records `id` as the remembered model for `profile` and persists
    /// the memory file. A failed write costs the memory, not the process
    /// (zone two).
    fn remember(&mut self, profile: String, id: String) {
        self.last_selected.insert(profile, id);
        if let Some(path) = &self.memory_path {
            store_memory(path, &self.last_selected);
        }
    }
}

/// A refused menu mutation. A refusal is a state to report, not an error
/// to escalate (zone two): the caller relays it and the applied state is
/// untouched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MenuRefusal {
    /// The requested model id is not in the current catalog.
    #[error("unknown model {id:?}: not in the current catalog")]
    UnknownModel {
        /// The id that was requested.
        id: String,
    },

    /// A profile switch is already in flight; switches are single-flight.
    #[error("a switch to {name:?} is already in progress")]
    SwitchInProgress {
        /// The target of the switch already running.
        name: String,
    },
}

/// How a profile switch ended, reported by whoever ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOutcome {
    /// The gateway finished loading the target profile.
    Completed,
    /// The switch failed; the previously active profile still serves.
    // An `allow` rather than an `expect`: the unit tests below construct
    // this in test builds, so an expectation would be unfulfilled there
    // and fail the -D warnings gate.
    #[allow(
        dead_code,
        reason = "the switch runner reports failures in a later step"
    )]
    Failed,
}

impl MenuBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot,
    /// loading the per-profile model memory from `state_dir` when one is
    /// given. A missing, unreadable, or corrupt memory file means "no
    /// memory yet": logged and tolerated (zone two), never fatal.
    pub(crate) fn new(catalog: CatalogBus, state_dir: Option<&Path>) -> Self {
        let memory_path = state_dir.map(|dir| dir.join(WORKSHOP_STATE_FILE));
        let last_selected = memory_path.as_deref().map(load_memory).unwrap_or_default();
        Self {
            sender: broadcast::channel(MENU_CHANNEL_CAPACITY).0,
            latest: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(MenuState {
                profiles: Vec::new(),
                active: None,
                switching: None,
                selected_model: None,
                gateway_reachable: false,
                last_selected,
                memory_path,
            })),
            catalog,
        }
    }

    /// Subscribes to every snapshot published from this call onward.
    // An `allow` rather than an `expect`: the unit tests below use this
    // in test builds, so an expectation would be unfulfilled there and
    // fail the -D warnings gate.
    #[allow(dead_code, reason = "the /ws session loop subscribes in a later step")]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WorkbenchSnapshot> {
        self.sender.subscribe()
    }

    /// The most recently published snapshot, retained so a session
    /// connecting later can send the current menu as its snapshot.
    pub(crate) fn latest(&self) -> Option<WorkbenchSnapshot> {
        // A lock poisoned by a panicking peer recovers the value rather
        // than wedging the process (the crate's zone-two error policy).
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Selects `id` as the chat model and publishes a fresh snapshot,
    /// remembering the choice for the active profile.
    ///
    /// # Errors
    /// Returns [`MenuRefusal::UnknownModel`] when `id` is not in the
    /// current catalog; the refused selection is not applied.
    #[allow(
        dead_code,
        reason = "the /ws session loop wires the mutators in a later step"
    )]
    pub(crate) fn set_selected(&self, id: &str) -> Result<(), MenuRefusal> {
        if !self.catalog_has(id) {
            return Err(MenuRefusal::UnknownModel { id: id.to_string() });
        }
        let mut state = self.lock_state();
        state.selected_model = Some(id.to_string());
        if let Some(profile) = state.active.clone() {
            state.remember(profile, id.to_string());
        }
        self.publish(&state);
        Ok(())
    }

    /// Marks a switch to profile `name` as in flight and publishes a
    /// fresh snapshot; `chat_ready` is false until the switch finishes.
    ///
    /// # Errors
    /// Returns [`MenuRefusal::SwitchInProgress`] while another switch
    /// runs - switches are single-flight because the gateway loads one
    /// profile at a time.
    #[allow(
        dead_code,
        reason = "the /ws session loop wires the mutators in a later step"
    )]
    pub(crate) fn begin_switch(&self, name: &str) -> Result<(), MenuRefusal> {
        let mut state = self.lock_state();
        if let Some(running) = &state.switching {
            return Err(MenuRefusal::SwitchInProgress {
                name: running.clone(),
            });
        }
        state.switching = Some(name.to_string());
        self.publish(&state);
        Ok(())
    }

    /// Ends the in-flight switch and publishes a fresh snapshot. On
    /// [`SwitchOutcome::Completed`] the target becomes the active profile
    /// and the selection moves to the remembered model for it when the
    /// catalog still holds that model, else to the first catalog model.
    /// On [`SwitchOutcome::Failed`] the previous profile stays active. A
    /// finish with no switch in flight is logged and ignored (zone two).
    #[allow(
        dead_code,
        reason = "the /ws session loop wires the mutators in a later step"
    )]
    pub(crate) fn finish_switch(&self, outcome: SwitchOutcome) {
        let mut state = self.lock_state();
        let Some(target) = state.switching.take() else {
            tracing::warn!("finish_switch with no switch in flight; ignored");
            return;
        };
        if outcome == SwitchOutcome::Completed {
            state.active = Some(target.clone());
            let models = self.catalog_models();
            state.selected_model = state
                .last_selected
                .get(&target)
                .filter(|id| models_contain(&models, id))
                .cloned()
                .or_else(|| first_model_id(&models));
            if let Some(id) = state.selected_model.clone() {
                state.remember(target, id);
            }
        }
        self.publish(&state);
    }

    /// Records the heartbeat's verdict on the gateway and publishes a
    /// fresh snapshot; `chat_ready` is false while the gateway is down.
    pub(crate) fn set_gateway_reachable(&self, reachable: bool) {
        let mut state = self.lock_state();
        state.gateway_reachable = reachable;
        self.publish(&state);
    }

    /// Records the gateway's profile list and active profile and
    /// publishes a fresh snapshot. The boot and reconnect paths feed
    /// this from the gateway's profile endpoints; a gateway without
    /// profile support feeds an empty list - a state, not an error.
    /// The selection is untouched: catalog reconciliation owns
    /// selection validity, not the profile list.
    pub(crate) fn set_profiles(&self, profiles: Vec<String>, active: Option<String>) {
        let mut state = self.lock_state();
        state.profiles = profiles;
        state.active = active;
        self.publish(&state);
    }

    /// Revalidates the selection against the current catalog - a selected
    /// model the catalog no longer holds is cleared - and republishes the
    /// snapshot when it changed. [`crate::push::Push::push_models_catalog`]
    /// calls this after every catalog publish, making that method the
    /// single choke point where catalog and menu reconcile.
    pub(crate) fn reconcile_catalog(&self) {
        let mut state = self.lock_state();
        if let Some(selected) = &state.selected_model
            && !self.catalog_has(selected)
        {
            state.selected_model = None;
        }
        let snapshot = self.snapshot(&state);
        if self.latest().as_ref() != Some(&snapshot) {
            self.send(snapshot);
        }
    }

    /// Publishes the current snapshot unconditionally, for producers that
    /// want the menu resent rather than changed.
    pub(crate) fn republish(&self) {
        let state = self.lock_state();
        self.publish(&state);
    }

    /// The state guard, recovering a lock poisoned by a panicking peer
    /// rather than wedging the process (the crate's zone-two policy).
    fn lock_state(&self) -> MutexGuard<'_, MenuState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the wire snapshot of `state`, computing `chat_ready` from
    /// its four conditions.
    fn snapshot(&self, state: &MenuState) -> WorkbenchSnapshot {
        let catalog_nonempty = self
            .catalog
            .latest()
            .is_some_and(|push| !push.models.is_empty());
        WorkbenchSnapshot {
            profiles: state.profiles.clone(),
            active: state.active.clone(),
            switching: state.switching.clone(),
            selected_model: state.selected_model.clone(),
            chat_ready: catalog_nonempty
                && state.selected_model.is_some()
                && state.switching.is_none()
                && state.gateway_reachable,
        }
    }

    /// Snapshots `state` and broadcasts it.
    fn publish(&self, state: &MenuState) {
        self.send(self.snapshot(state));
    }

    /// Broadcasts one snapshot. With no subscribers this is a no-op; a
    /// slow subscriber skips ahead rather than applying backpressure.
    fn send(&self, snapshot: WorkbenchSnapshot) {
        // The retained copy (a second owner, hence the clone) is written
        // before the send, so a session that subscribes after the send
        // still finds this snapshot.
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(snapshot.clone());
        // A send only fails when there are no receivers, which is the
        // bus's resting state before the first client connects.
        let _ = self.sender.send(snapshot);
    }

    /// Whether `id` names a model in the current catalog snapshot.
    fn catalog_has(&self, id: &str) -> bool {
        self.catalog
            .latest()
            .is_some_and(|push| models_contain(&push.models, id))
    }

    /// The current catalog's models array, empty before the first push.
    fn catalog_models(&self) -> Vec<serde_json::Value> {
        self.catalog
            .latest()
            .map_or_else(Vec::new, |push| push.models)
    }
}

/// Whether the catalog `models` array holds an entry whose `id` is `id`.
fn models_contain(models: &[serde_json::Value], id: &str) -> bool {
    models
        .iter()
        .any(|model| model.get("id").and_then(serde_json::Value::as_str) == Some(id))
}

/// The `id` of the first catalog entry carrying one, when any does.
fn first_model_id(models: &[serde_json::Value]) -> Option<String> {
    models.iter().find_map(|model| {
        model
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// The persisted shape of [`WORKSHOP_STATE_FILE`]. Server state only:
/// the UI's panel layout is view state and stays in webview localStorage.
#[derive(Debug, Default, serde::Deserialize)]
struct StoredState {
    /// Remembered model selection per profile name.
    #[serde(default)]
    last_selected: HashMap<String, String>,
}

/// Loads the per-profile model memory. A missing, unreadable, or corrupt
/// file means "no memory yet": logged and tolerated (zone two).
fn load_memory(path: &Path) -> HashMap<String, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "workshop state unreadable; starting with no model memory"
                );
            }
            return HashMap::new();
        }
    };
    match serde_json::from_str::<StoredState>(&raw) {
        Ok(stored) => stored.last_selected,
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "workshop state corrupt; starting with no model memory"
            );
            HashMap::new()
        }
    }
}

/// Writes the per-profile model memory. A failed write costs the memory,
/// not the process (zone two): logged and tolerated.
fn store_memory(path: &Path, last_selected: &HashMap<String, String>) {
    let payload = serde_json::json!({ "last_selected": last_selected });
    let text = payload.to_string();
    if let Err(error) = std::fs::write(path, text) {
        tracing::warn!(
            %error,
            path = %path.display(),
            "workshop state write failed; model memory not persisted"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    /// A catalog bus already holding one push of the given model ids.
    fn catalog_of(ids: &[&str]) -> CatalogBus {
        let catalog = CatalogBus::new();
        catalog.publish(ids.iter().map(|id| serde_json::json!({"id": id})).collect());
        catalog
    }

    /// A menu with no persistence, on a catalog of the given model ids.
    fn menu_of(ids: &[&str]) -> MenuBus {
        MenuBus::new(catalog_of(ids), None)
    }

    /// Drives `menu` to full readiness on `profile`: gateway reachable
    /// and a completed switch, which selects a model.
    fn onto_profile(menu: &MenuBus, profile: &str) {
        menu.set_gateway_reachable(true);
        menu.begin_switch(profile).expect("no switch is running");
        menu.finish_switch(SwitchOutcome::Completed);
    }

    /// The retained snapshot, which every mutation publishes.
    fn snapshot(menu: &MenuBus) -> WorkbenchSnapshot {
        menu.latest().expect("a mutation published a snapshot")
    }

    #[test]
    fn a_known_model_selects_and_publishes_the_snapshot() {
        let menu = menu_of(&["model-a", "model-b"]);
        menu.set_selected("model-b")
            .expect("the id is in the catalog");
        assert_eq!(snapshot(&menu).selected_model.as_deref(), Some("model-b"));
    }

    #[test]
    fn an_unknown_model_is_refused_and_not_applied() {
        let menu = menu_of(&["model-a"]);
        menu.set_selected("model-a")
            .expect("the id is in the catalog");
        let refusal = menu
            .set_selected("model-x")
            .expect_err("an unknown id is refused");
        assert_eq!(
            refusal,
            MenuRefusal::UnknownModel {
                id: "model-x".to_string()
            }
        );
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "a refused selection leaves the applied one in place"
        );
    }

    #[test]
    fn a_switch_publishes_its_begin_and_its_finish() {
        let menu = menu_of(&["model-a"]);
        menu.set_gateway_reachable(true);
        menu.begin_switch("coding").expect("no switch is running");
        let during = snapshot(&menu);
        assert_eq!(during.switching.as_deref(), Some("coding"));
        assert!(!during.chat_ready, "a switch in flight blocks chat");
        menu.finish_switch(SwitchOutcome::Completed);
        let after = snapshot(&menu);
        assert_eq!(after.active.as_deref(), Some("coding"));
        assert_eq!(after.switching, None);
        assert_eq!(
            after.selected_model.as_deref(),
            Some("model-a"),
            "with no memory for the profile the first catalog model is selected"
        );
        assert!(after.chat_ready);
    }

    #[test]
    fn a_failed_switch_keeps_the_previous_profile() {
        let menu = menu_of(&["model-a"]);
        onto_profile(&menu, "main");
        menu.begin_switch("coding").expect("no switch is running");
        menu.finish_switch(SwitchOutcome::Failed);
        let after = snapshot(&menu);
        assert_eq!(after.active.as_deref(), Some("main"));
        assert_eq!(after.switching, None);
        assert!(after.chat_ready, "the previous profile still serves");
    }

    #[test]
    fn a_second_switch_while_one_runs_is_refused() {
        let menu = menu_of(&["model-a"]);
        menu.begin_switch("coding").expect("no switch is running");
        let refusal = menu
            .begin_switch("writing")
            .expect_err("switches are single-flight");
        assert_eq!(
            refusal,
            MenuRefusal::SwitchInProgress {
                name: "coding".to_string()
            }
        );
        assert_eq!(
            snapshot(&menu).switching.as_deref(),
            Some("coding"),
            "the refused switch is not applied"
        );
    }

    #[test]
    fn finishing_with_no_switch_in_flight_is_tolerated() {
        let menu = menu_of(&[]);
        menu.finish_switch(SwitchOutcome::Completed);
        assert!(menu.latest().is_none(), "a no-op finish publishes nothing");
    }

    #[test]
    fn the_newest_snapshot_is_retained_for_the_connect_snapshot() {
        let menu = menu_of(&["model-a", "model-b"]);
        assert!(menu.latest().is_none(), "an untouched menu has no snapshot");
        menu.set_selected("model-a")
            .expect("the id is in the catalog");
        menu.set_selected("model-b")
            .expect("the id is in the catalog");
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-b"),
            "a session connecting now snapshots the newest state"
        );
    }

    #[tokio::test]
    async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
        let menu = menu_of(&["model-a"]);
        let mut receiver = menu.subscribe();
        for _ in 0..=MENU_CHANNEL_CAPACITY {
            menu.set_gateway_reachable(true);
        }
        match receiver.recv().await {
            Err(RecvError::Lagged(1)) => {}
            other => panic!("expected a lag report of one, got {other:?}"),
        }
        receiver
            .recv()
            .await
            .expect("the ring still holds snapshots");
    }

    #[test]
    fn a_catalog_change_clears_a_selection_it_no_longer_holds() {
        let catalog = catalog_of(&["model-a"]);
        let menu = MenuBus::new(catalog.clone(), None);
        menu.set_selected("model-a")
            .expect("the id is in the catalog");
        catalog.publish(vec![serde_json::json!({"id": "model-b"})]);
        menu.reconcile_catalog();
        assert_eq!(
            snapshot(&menu).selected_model,
            None,
            "the vanished selection is revalidated away"
        );
    }

    #[test]
    fn a_catalog_change_that_keeps_the_selection_republishes_nothing() {
        let catalog = catalog_of(&["model-a"]);
        let menu = MenuBus::new(catalog.clone(), None);
        menu.set_selected("model-a")
            .expect("the id is in the catalog");
        let mut receiver = menu.subscribe();
        catalog.publish(vec![
            serde_json::json!({"id": "model-a"}),
            serde_json::json!({"id": "model-b"}),
        ]);
        menu.reconcile_catalog();
        assert!(
            matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
            "an unchanged snapshot is not republished"
        );
    }

    #[test]
    fn chat_ready_is_true_only_when_every_condition_holds() {
        let catalog = catalog_of(&["model-a"]);
        let menu = MenuBus::new(catalog.clone(), None);
        onto_profile(&menu, "main");
        assert!(
            snapshot(&menu).chat_ready,
            "non-empty catalog, a selection, no switch, gateway up"
        );

        menu.set_gateway_reachable(false);
        assert!(!snapshot(&menu).chat_ready, "gateway down forces false");
        menu.set_gateway_reachable(true);

        menu.begin_switch("coding").expect("no switch is running");
        assert!(
            !snapshot(&menu).chat_ready,
            "a switch in flight forces false"
        );
        menu.finish_switch(SwitchOutcome::Completed);
        assert!(
            snapshot(&menu).chat_ready,
            "the finished switch restores readiness"
        );

        catalog.publish(vec![serde_json::json!({"id": "model-b"})]);
        menu.reconcile_catalog();
        let cleared = snapshot(&menu);
        assert_eq!(cleared.selected_model, None);
        assert!(!cleared.chat_ready, "no selection forces false");

        menu.set_selected("model-b")
            .expect("the id is in the catalog");
        catalog.publish(Vec::new());
        menu.reconcile_catalog();
        assert!(!snapshot(&menu).chat_ready, "an empty catalog forces false");
    }

    #[test]
    fn set_profiles_publishes_the_list_and_the_active_profile() {
        let menu = menu_of(&["model-a"]);
        menu.set_profiles(
            vec!["main".to_string(), "coding".to_string()],
            Some("main".to_string()),
        );
        let published = snapshot(&menu);
        assert_eq!(published.profiles, ["main", "coding"]);
        assert_eq!(published.active.as_deref(), Some("main"));
    }

    #[test]
    fn set_profiles_leaves_the_selection_and_readiness_alone() {
        let menu = menu_of(&["model-a"]);
        onto_profile(&menu, "main");
        menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
        let after = snapshot(&menu);
        assert_eq!(
            after.selected_model.as_deref(),
            Some("model-a"),
            "the profile list does not own selection validity"
        );
        assert!(after.chat_ready, "readiness survives a profile refresh");
    }

    #[test]
    fn an_empty_profile_list_replaces_a_populated_one() {
        let menu = menu_of(&[]);
        menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
        menu.set_profiles(Vec::new(), None);
        let after = snapshot(&menu);
        assert!(
            after.profiles.is_empty(),
            "a gateway without profile support publishes an empty list"
        );
        assert_eq!(after.active, None);
    }

    #[test]
    fn a_selection_is_remembered_per_profile_across_switches() {
        let menu = menu_of(&["model-a", "model-b", "model-c"]);
        onto_profile(&menu, "main");
        menu.set_selected("model-c")
            .expect("the id is in the catalog");
        menu.begin_switch("coding").expect("no switch is running");
        menu.finish_switch(SwitchOutcome::Completed);
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "a profile with no memory selects the first model"
        );
        menu.set_selected("model-b")
            .expect("the id is in the catalog");
        menu.begin_switch("main").expect("no switch is running");
        menu.finish_switch(SwitchOutcome::Completed);
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-c"),
            "the remembered model for the profile is restored"
        );
    }

    #[test]
    fn model_memory_round_trips_through_the_state_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let catalog = catalog_of(&["model-a", "model-b"]);
        {
            let menu = MenuBus::new(catalog.clone(), Some(dir.path()));
            onto_profile(&menu, "main");
            menu.set_selected("model-b")
                .expect("the id is in the catalog");
        }
        let reborn = MenuBus::new(catalog, Some(dir.path()));
        onto_profile(&reborn, "main");
        assert_eq!(
            snapshot(&reborn).selected_model.as_deref(),
            Some("model-b"),
            "the persisted memory survives a restart"
        );
    }

    #[test]
    fn a_missing_state_file_means_no_memory_yet() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
        onto_profile(&menu, "main");
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "no memory yet: the first catalog model is selected"
        );
    }

    #[test]
    fn a_corrupt_state_file_means_no_memory_yet() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join(WORKSHOP_STATE_FILE), "not json {").expect("write fixture");
        let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
        onto_profile(&menu, "main");
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "corrupt memory degrades to no memory, never to a failure"
        );
    }

    #[test]
    fn an_unreadable_state_file_means_no_memory_yet() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // A directory in the file's place: reads and writes both fail for
        // a reason other than NotFound, and both must degrade.
        std::fs::create_dir(dir.path().join(WORKSHOP_STATE_FILE))
            .expect("directory in the file's place");
        let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
        onto_profile(&menu, "main");
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "unreadable memory degrades to no memory, never to a failure"
        );
    }

    #[test]
    fn a_remembered_model_gone_from_the_catalog_falls_back_to_the_first() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            dir.path().join(WORKSHOP_STATE_FILE),
            r#"{"last_selected":{"main":"retired-model"}}"#,
        )
        .expect("write fixture");
        let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
        onto_profile(&menu, "main");
        assert_eq!(
            snapshot(&menu).selected_model.as_deref(),
            Some("model-a"),
            "a remembered model the catalog no longer holds falls back to the first"
        );
    }
}
