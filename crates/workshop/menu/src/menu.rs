//! The server-owned Model menu: the workbench snapshot pushed to every
//! `/ws` session as a `{"type":"workbench",...}` frame, its broadcast
//! bus, and the per-profile model memory persisted in the state directory.
//!
//! The server owns all Model-menu state and the UI only renders it; in
//! particular `chat_ready` is computed here - a chat-capable model
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

#[path = "menu-memory.rs"]
mod memory;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use workshop_protocol::WorkbenchSnapshot;
use workshop_support::RetainedBus;

use crate::catalog::{CatalogBus, is_chat_capable};

use self::memory::{PendingWrite, WORKSHOP_STATE_FILE, load_memory, store_pending};

/// Ring capacity of the menu bus. Pushes follow user interactions and
/// heartbeat transitions, so a handful of slots is generous.
const MENU_CHANNEL_CAPACITY: usize = 8;

/// The shared menu bus: the Model-menu state, its mutators, and the
/// broadcast channel their snapshots fan out on, mirroring
/// [`crate::catalog::CatalogBus`].
///
/// Clones are cheap (a few `Arc` bumps) and share one state, one retained
/// snapshot, and one channel.
#[derive(Debug, Clone)]
pub struct MenuBus {
    bus: RetainedBus<WorkbenchSnapshot>,
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
    /// The selection a switch is applying, while one is in flight.
    switching: Option<SwitchTarget>,
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

/// What an in-flight switch selects: a named profile, or no profile at
/// all. Distinct from "no switch running", which is the absence of a
/// target.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SwitchTarget {
    /// A defined profile, by name.
    Profile(String),
    /// No profile: the gateway serves remote models only.
    NoProfile,
}

impl SwitchTarget {
    /// The target as the optional profile name the wire and the active
    /// state carry.
    fn name(&self) -> Option<&str> {
        match self {
            Self::Profile(name) => Some(name),
            Self::NoProfile => None,
        }
    }
}

/// Renders an optional profile name for a refusal message.
fn describe_target(name: Option<&str>) -> String {
    name.map_or_else(|| "no profile".to_owned(), |name| format!("{name:?}"))
}

impl MenuState {
    /// Applies the remembered-else-first selection rule for `profile`:
    /// the remembered model when `models` still holds it, else the first
    /// catalog model. Records the choice in per-profile memory and
    /// returns its pending write when a model was selected. Shared by
    /// [`MenuBus::finish_switch`] and [`MenuBus::restore_selection`].
    #[must_use]
    fn select_for_profile(
        &mut self,
        profile: String,
        models: &[serde_json::Value],
    ) -> Option<PendingWrite> {
        self.selected_model = self
            .last_selected
            .get(&profile)
            .filter(|id| models_contain(models, id))
            .cloned()
            .or_else(|| first_model_id(models));
        let id = self.selected_model.clone()?;
        self.remember(profile, id)
    }

    /// Records `id` as the remembered model for `profile` and snapshots
    /// the serialized memory as a [`PendingWrite`] for the caller to
    /// perform once the state lock is released - the mutators run on the
    /// async runtime, and file IO under the guard would block the
    /// executor. `None` when persistence is disabled.
    #[must_use]
    fn remember(&mut self, profile: String, id: String) -> Option<PendingWrite> {
        self.last_selected.insert(profile, id);
        let path = self.memory_path.clone()?;
        let payload = serde_json::json!({ "last_selected": &self.last_selected });
        Some(PendingWrite {
            path,
            bytes: payload.to_string().into_bytes(),
        })
    }
}

/// A refused menu mutation. A refusal is a state to report, not an error
/// to escalate (zone two): the caller relays it and the applied state is
/// untouched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MenuRefusal {
    /// The requested model id is not in the current catalog.
    #[error("unknown model {id:?}: not in the current catalog")]
    #[non_exhaustive]
    UnknownModel {
        /// The id that was requested.
        id: String,
    },

    /// A profile switch is already in flight; switches are single-flight.
    #[error("a switch to {} is already in progress", describe_target(.name.as_deref()))]
    #[non_exhaustive]
    SwitchInProgress {
        /// The target of the switch already running: a profile name, or
        /// `None` for a switch to no profile.
        name: Option<String>,
    },
}

/// How a profile switch ended, reported by whoever ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The gateway serves the target selection.
    Completed,
    /// The selection persisted but the gateway must restart to load it;
    /// the previously active profile still serves.
    Deferred,
    /// The switch failed; the previously active profile still serves.
    Failed,
}

impl MenuBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot,
    /// loading the per-profile model memory from `state_dir` when one is
    /// given. A missing, unreadable, or corrupt memory file means "no
    /// memory yet": logged and tolerated (zone two), never fatal.
    #[must_use]
    pub fn new(catalog: CatalogBus, state_dir: Option<&Path>) -> Self {
        let memory_path = state_dir.map(|dir| dir.join(WORKSHOP_STATE_FILE));
        let last_selected = memory_path.as_deref().map(load_memory).unwrap_or_default();
        Self {
            bus: RetainedBus::new(MENU_CHANNEL_CAPACITY),
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
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<WorkbenchSnapshot> {
        self.bus.subscribe()
    }

    /// The most recently published snapshot, retained so a session
    /// connecting later can send the current menu as its snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<WorkbenchSnapshot> {
        self.bus.latest()
    }

    /// Selects `id` as the chat model and publishes a fresh snapshot,
    /// remembering the choice for the active profile.
    ///
    /// # Errors
    /// Returns [`MenuRefusal::UnknownModel`] when `id` is not in the
    /// current catalog; the refused selection is not applied.
    pub fn set_selected(&self, id: &str) -> Result<(), MenuRefusal> {
        if !self.catalog_has(id) {
            return Err(MenuRefusal::UnknownModel { id: id.to_string() });
        }
        let mut state = self.lock_state();
        state.selected_model = Some(id.to_string());
        let pending = state
            .active
            .clone()
            .and_then(|profile| state.remember(profile, id.to_string()));
        self.publish(&state);
        drop(state);
        store_pending(pending);
        Ok(())
    }

    /// Marks a switch to profile `name` - or to no profile, for `None` -
    /// as in flight and publishes a fresh snapshot; `chat_ready` is false
    /// until the switch finishes. The wire snapshot names the target
    /// profile while one is being selected and carries `null` for a
    /// switch to no profile; `switch_in_flight` is true for either.
    ///
    /// # Errors
    /// Returns [`MenuRefusal::SwitchInProgress`] while another switch
    /// runs - switches are single-flight because the gateway applies one
    /// selection at a time.
    pub fn begin_switch(&self, name: Option<&str>) -> Result<(), MenuRefusal> {
        let mut state = self.lock_state();
        if let Some(running) = &state.switching {
            return Err(MenuRefusal::SwitchInProgress {
                name: running.name().map(str::to_owned),
            });
        }
        state.switching = Some(name.map_or(SwitchTarget::NoProfile, |name| {
            SwitchTarget::Profile(name.to_owned())
        }));
        self.publish(&state);
        Ok(())
    }

    /// Ends the in-flight switch and publishes a fresh snapshot. On
    /// [`SwitchOutcome::Completed`] the target becomes the active
    /// selection and the model selection moves to the remembered model
    /// for the target profile when the catalog still holds that model,
    /// else to the first catalog model; a switch to no profile has no
    /// memory to consult and selects the first catalog model. On
    /// [`SwitchOutcome::Deferred`] and [`SwitchOutcome::Failed`] the
    /// previous profile stays active. A finish with no switch in flight
    /// is logged and ignored (zone two).
    pub fn finish_switch(&self, outcome: SwitchOutcome) {
        let mut state = self.lock_state();
        let Some(target) = state.switching.take() else {
            tracing::warn!("finish_switch with no switch in flight; ignored");
            return;
        };
        let mut pending = None;
        if outcome == SwitchOutcome::Completed {
            let models = self.catalog_models();
            match target {
                SwitchTarget::Profile(profile) => {
                    state.active = Some(profile.clone());
                    pending = state.select_for_profile(profile, &models);
                }
                SwitchTarget::NoProfile => {
                    state.active = None;
                    state.selected_model = first_model_id(&models);
                }
            }
        }
        self.publish(&state);
        drop(state);
        store_pending(pending);
    }

    /// Restores a boot-time selection: when nothing is selected and the
    /// catalog holds a model, selects the remembered model for the
    /// active profile when the catalog still holds it, else the first
    /// catalog model - the rule [`MenuBus::finish_switch`] applies - and
    /// publishes a fresh snapshot. With a selection already applied, or
    /// no selectable catalog model, this is a no-op and publishes
    /// nothing. The heartbeat calls this after its boot and reconnect
    /// refreshes settle, so a reconnect whose selection survived the
    /// outage changes nothing.
    pub fn restore_selection(&self) {
        let mut state = self.lock_state();
        if state.selected_model.is_some() {
            return;
        }
        let models = self.catalog_models();
        let pending = if let Some(profile) = state.active.clone() {
            state.select_for_profile(profile, &models)
        } else {
            // No active profile means no memory to consult and none to
            // record; fall straight back to the first catalog model.
            state.selected_model = first_model_id(&models);
            None
        };
        if state.selected_model.is_none() {
            return;
        }
        self.publish(&state);
        drop(state);
        store_pending(pending);
    }

    /// Records the heartbeat's verdict on the gateway and publishes a
    /// fresh snapshot; `chat_ready` is false while the gateway is down.
    pub fn set_gateway_reachable(&self, reachable: bool) {
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
    pub fn set_profiles(&self, profiles: Vec<String>, active: Option<String>) {
        let mut state = self.lock_state();
        state.profiles = profiles;
        state.active = active;
        self.publish(&state);
    }

    /// Revalidates the selection against the current catalog - a selected
    /// model the catalog no longer holds is cleared - and republishes the
    /// snapshot when it changed.
    /// [`workshop_registry::Push::push_models_catalog`] calls this after
    /// every catalog publish, making that method the single choke point
    /// where catalog and menu reconcile.
    pub fn reconcile_catalog(&self) {
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

    /// Revalidates the selection after an integration fixture publishes
    /// directly to the catalog bus.
    #[cfg(feature = "test-fixtures")]
    pub fn reconcile_catalog_for_test(&self) {
        self.reconcile_catalog();
    }

    /// The state guard, recovering a lock poisoned by a panicking peer
    /// rather than wedging the process (the crate's zone-two policy).
    fn lock_state(&self) -> MutexGuard<'_, MenuState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the wire snapshot of `state`, computing `chat_ready` from
    /// its four conditions.
    fn snapshot(&self, state: &MenuState) -> WorkbenchSnapshot {
        let catalog_has_chat = self
            .catalog
            .latest()
            .is_some_and(|push| push.models.iter().any(is_chat_capable));
        WorkbenchSnapshot {
            profiles: state.profiles.clone(),
            active: state.active.clone(),
            switching: state
                .switching
                .as_ref()
                .and_then(|target| target.name().map(str::to_owned)),
            switch_in_flight: state.switching.is_some(),
            selected_model: state.selected_model.clone(),
            chat_ready: catalog_has_chat
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
        self.bus.send(snapshot);
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
    models.iter().any(|model| {
        is_chat_capable(model) && model.get("id").and_then(serde_json::Value::as_str) == Some(id)
    })
}

/// The `id` of the first chat-capable catalog entry, when any does.
fn first_model_id(models: &[serde_json::Value]) -> Option<String> {
    models
        .iter()
        .filter(|model| is_chat_capable(model))
        .find_map(|model| {
            model
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

#[cfg(test)]
#[path = "menu-tests.rs"]
mod tests;
