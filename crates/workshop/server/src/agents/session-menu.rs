//! The socket side of the Model menu: `select_model` and
//! `switch_profile` frame handling, plus the profile-selection task that
//! persists the selection on the gateway, restarts a supervised sidecar
//! to load it, and holds the status bar busy until it settles. The menu
//! state and bus sit in the menu subsystem (`workshop-menu`); this
//! module is only the session's orchestration of them.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::WebSocket;

use workshop_gateway::heartbeat::{refresh_catalog, refresh_profiles};
use workshop_gateway::{
    GatewayClient, GatewayError, GatewayResponse, GatewaySnapshot, SwitchResponse,
};
use workshop_menu::{MenuBus, SwitchOutcome};
use workshop_protocol::{Activity, SwitchProfileFrame};
use workshop_registry::Push;

use crate::agents::relay::value_from_bytes;
use crate::agents::state::SessionsState;

use super::send_error;

/// Handles a `select_model` frame: the menu validates the id against
/// the retained catalog and publishes the fresh workbench snapshot,
/// which reaches this client through its own menu branch. Handled
/// inline in the session loop - a map lookup and a broadcast send cost
/// microseconds between two deltas. A refusal (an unknown model, a
/// missing field) is answered with an `error` frame and the session
/// continues (zone two).
pub(super) async fn select_model(
    state: &SessionsState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Some(model) = frame.get("model").and_then(serde_json::Value::as_str) else {
        send_error(socket, id, "select_model needs a \"model\" string").await;
        return;
    };
    let Some(menu) = state.menu() else {
        send_error(socket, id, "the model menu is unavailable").await;
        return;
    };
    if let Err(refusal) = menu.set_selected(model) {
        send_error(socket, id, refusal.to_string()).await;
    }
}

/// Handles a `switch_profile` frame: `begin_switch` publishes the
/// pending snapshot (`switching` set, `chat_ready` false) before this
/// returns, and the selection itself runs on its own task. A refusal (a
/// switch already in flight, a missing or mistyped `name`) is answered
/// with an `error` frame and the session continues (zone two).
pub(super) async fn start_switch(
    state: &SessionsState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Ok(request) = serde_json::from_value::<SwitchProfileFrame>(frame.clone()) else {
        send_error(
            socket,
            id,
            "switch_profile needs a \"name\" string, or null for no profile",
        )
        .await;
        return;
    };
    let Some(menu) = state.menu() else {
        send_error(socket, id, "the model menu is unavailable").await;
        return;
    };
    let Some(snapshot) = state.gateway_snapshot() else {
        send_error(socket, id, "the gateway is unavailable").await;
        return;
    };
    if let Err(refusal) = menu.begin_switch(request.name.as_deref()) {
        send_error(socket, id, refusal.to_string()).await;
        return;
    }
    // Deliberately not client-scoped - a stated exception to the crate's
    // drop-guard cancellation rule: a profile selection is global server
    // state, not work held on behalf of one client, so it runs to
    // completion (and settles the menu) even if the clicking client
    // disconnects mid-switch.
    let push = state.push();
    let state = state.clone();
    tokio::spawn(async move {
        run_switch(&state, snapshot, &push, &menu, request.name.as_deref()).await;
    });
}

/// The status-bar text while a selection runs; the bar stays busy under
/// it until the switch settles with its own terminal frame.
const SWITCHING_LABEL: &str = "Switching profile...";

/// How often the ladder re-reads the published gateway generation while
/// waiting for the relaunched sidecar.
const REPLACEMENT_POLL: Duration = Duration::from_millis(250);

/// Runs one profile selection to its end and settles the menu: the
/// selection persisted and served (through a sidecar restart when the
/// gateway asks for one) refetches the profile state and model catalog
/// through the serving gateway and completes; a selection a LAN gateway
/// must be restarted by hand to load settles deferred, the running
/// profile unchanged; a failure restores the truthful pre-switch state
/// and reports itself.
///
/// The status bar goes busy once, here at the start; every settled arm
/// ends with a non-busy frame (idle, the deferred notice, or the
/// failure), so no separate idle push is needed.
async fn run_switch(
    state: &SessionsState,
    snapshot: Arc<GatewaySnapshot>,
    push: &Push,
    menu: &MenuBus,
    name: Option<&str>,
) {
    push.push_busy(
        SWITCHING_LABEL,
        format!("switching to {}", describe(name)),
        Activity::General,
    );
    match drive_switch(state, &snapshot, name).await {
        Ok(Settled::Serving(client)) => {
            // The settled snapshot reads a fresh catalog and profile list
            // from the gateway that now serves the selection.
            tokio::join!(
                refresh_profiles(&client, push),
                refresh_catalog(&client, push)
            );
            menu.finish_switch(SwitchOutcome::Completed);
            push.push_idle();
        }
        Ok(Settled::RestartRequired) => {
            menu.finish_switch(SwitchOutcome::Deferred);
            push.push_status_update("Profile selected", deferred_notice(name), Activity::General);
        }
        Err(failure) => {
            // A gateway that refused or dropped the selection still
            // serves, and its state may have changed, so the menu
            // refetches before it settles. A sidecar that was shut down
            // and never came back has nothing truthful to fetch: the
            // heartbeat reports the outage and repopulates the menu when
            // a generation does appear.
            if failure.gateway_serves()
                && let Some(current) = state.gateway_snapshot()
            {
                tokio::join!(
                    refresh_profiles(current.client(), push),
                    refresh_catalog(current.client(), push)
                );
            }
            menu.finish_switch(SwitchOutcome::Failed);
            push.push_failure(
                "Profile switch failed",
                failure.to_string(),
                Activity::General,
            );
        }
    }
}

/// How a selection ended short of failure.
enum Settled {
    /// The gateway behind `client` serves the selection.
    Serving(GatewayClient),
    /// The selection persisted, but the gateway is not a supervised
    /// sidecar: the operator restarts it by hand.
    RestartRequired,
}

/// Why one profile selection did not complete. The display text is the
/// user-facing description pushed with the failure status, so each
/// variant renders exactly the message the stringly channel reported.
#[derive(Debug, thiserror::Error)]
enum SwitchFailure {
    /// The selection request failed in transit; the client's typed error
    /// is retained as the cause.
    #[error(transparent)]
    Transport(GatewayError),
    /// The gateway refused the selection; the payload is the gateway's
    /// own refusal message, relayed verbatim.
    #[error("{0}")]
    Refused(String),
    /// The sidecar refused or never received its shutdown request.
    #[error("the gateway did not accept its shutdown request: {0}")]
    Shutdown(String),
    /// No replacement gateway serving the selection appeared in time.
    #[error("gateway did not return after restart")]
    RestartTimeout,
}

impl SwitchFailure {
    /// Whether the published gateway generation still serves after this
    /// failure: true before any shutdown went out, false once the sidecar
    /// was asked to exit.
    fn gateway_serves(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::Refused(_))
    }
}

/// Posts the selection and climbs the ladder: `Serving` at once when the
/// gateway needs no restart, `RestartRequired` when it does but is not a
/// supervised sidecar, else the shutdown-and-reappear step whose
/// replacement generation ends up `Serving`.
async fn drive_switch(
    state: &SessionsState,
    snapshot: &Arc<GatewaySnapshot>,
    name: Option<&str>,
) -> Result<Settled, SwitchFailure> {
    let outcome = match snapshot.client().switch_profile(name).await {
        Ok(SwitchResponse::Selected(outcome)) => outcome,
        Ok(SwitchResponse::Buffered(refusal)) => {
            return Err(SwitchFailure::Refused(switch_refusal(&refusal)));
        }
        // A variant this build does not know: the gateway may grow
        // response shapes, and a lost selection never degrades the server.
        Ok(_) => {
            return Err(SwitchFailure::Refused(
                "unrecognized gateway answer".to_owned(),
            ));
        }
        Err(error) => return Err(SwitchFailure::Transport(error)),
    };
    if !outcome.restart_required {
        return Ok(Settled::Serving(snapshot.client().clone()));
    }
    if !snapshot.is_sidecar() {
        return Ok(Settled::RestartRequired);
    }
    let generation = snapshot.generation();
    // The shutdown request is blocking I/O against the sidecar; the
    // supervisor relaunches the sibling once the process exits.
    let shutdown = Arc::clone(snapshot);
    match tokio::task::spawn_blocking(move || shutdown.request_shutdown()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return Err(SwitchFailure::Shutdown(error.to_string())),
        Err(join) => return Err(SwitchFailure::Shutdown(join.to_string())),
    }
    // The captured pre-restart client is never used past this point: the
    // relaunched sidecar binds a fresh port and key, published as a new
    // generation by the supervisor.
    let replacement = await_replacement(state, generation, name).await?;
    Ok(Settled::Serving(replacement.client().clone()))
}

/// Waits for a published generation newer than `generation` whose status
/// reports `target` as the served profile, within the state's restart
/// bound.
async fn await_replacement(
    state: &SessionsState,
    generation: u64,
    target: Option<&str>,
) -> Result<Arc<GatewaySnapshot>, SwitchFailure> {
    let converged = async {
        loop {
            if let Some(candidate) = state.gateway_snapshot()
                && candidate.generation() > generation
                && served_profile(candidate.client())
                    .await
                    .as_ref()
                    .map(Option::as_deref)
                    == Some(target)
            {
                return candidate;
            }
            tokio::time::sleep(REPLACEMENT_POLL).await;
        }
    };
    tokio::time::timeout(state.restart_bound(), converged)
        .await
        .map_err(|_| SwitchFailure::RestartTimeout)
}

/// The profile `GET /admin/status` reports as served - `Some(None)` for
/// a gateway serving no profile - or `None` when the gateway did not
/// answer with a status document.
async fn served_profile(client: &GatewayClient) -> Option<Option<String>> {
    let response = client.profile_status().await.ok()?;
    if !response.status.is_success() {
        return None;
    }
    let body: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    let profile = body.get("profile")?;
    if profile.is_null() {
        return Some(None);
    }
    profile.as_str().map(|name| Some(name.to_owned()))
}

/// The notice for a selection a LAN gateway persisted but must be
/// restarted by hand to apply: a named profile is loaded by the restart,
/// no profile unloads whatever the gateway is running.
fn deferred_notice(name: Option<&str>) -> String {
    name.map_or_else(
        || "no profile selected; restart the gateway to unload the running profile".to_owned(),
        |name| format!("profile {name:?} selected; restart the gateway to load it"),
    )
}

/// Renders the selection for status text: the quoted profile name, or
/// `no profile`.
fn describe(name: Option<&str>) -> String {
    name.map_or_else(|| "no profile".to_owned(), |name| format!("{name:?}"))
}

/// The failure description of a buffered switch refusal: the gateway's
/// own error message when its envelope has one, else the status.
fn switch_refusal(refusal: &GatewayResponse) -> String {
    value_from_bytes(&refusal.body)
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || format!("gateway declined the switch with status {}", refusal.status),
            str::to_string,
        )
}
