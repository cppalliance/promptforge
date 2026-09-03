//! The socket side of the Model menu: `select_model` and
//! `switch_profile` frame handling, plus the profile-switch task that
//! drives the gateway's stage stream into status-bar progress. The menu
//! state and bus live in the crate-root [`crate::menu`]; this module is
//! only the session's orchestration of them.

use axum::extract::ws::WebSocket;
use futures_util::StreamExt;

use crate::app::AppState;
use crate::gateway::{GatewayClient, GatewayResponse, SwitchEvent, SwitchResponse, switch_events};
use crate::heartbeat::{refresh_catalog, refresh_profiles};
use crate::menu::SwitchOutcome;
use crate::protocol::Activity;
use crate::push::Push;
use crate::relay::value_from_bytes;

use super::send_error;

/// Handles a `select_model` frame: the menu validates the id against
/// the retained catalog and publishes the fresh workbench snapshot,
/// which reaches this client through its own menu branch. Handled
/// inline in the session loop - a map lookup and a broadcast send cost
/// microseconds between two deltas. A refusal (an unknown model, a
/// missing field) is answered with an `error` frame and the session
/// continues (zone two).
pub(super) async fn select_model(
    state: &AppState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Some(model) = frame.get("model").and_then(serde_json::Value::as_str) else {
        send_error(socket, id, "select_model needs a \"model\" string").await;
        return;
    };
    if let Err(refusal) = state.menu().set_selected(model) {
        send_error(socket, id, refusal.to_string()).await;
    }
}

/// Handles a `switch_profile` frame: `begin_switch` publishes the
/// pending snapshot (`switching` set, `chat_ready` false) before this
/// returns, and the switch itself runs on its own task. A refusal (a
/// switch already in flight, a missing field) is answered with an
/// `error` frame and the session continues (zone two).
pub(super) async fn start_switch(
    state: &AppState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Some(name) = frame.get("name").and_then(serde_json::Value::as_str) else {
        send_error(socket, id, "switch_profile needs a \"name\" string").await;
        return;
    };
    if let Err(refusal) = state.menu().begin_switch(name) {
        send_error(socket, id, refusal.to_string()).await;
        return;
    }
    // Deliberately not client-scoped - a stated exception to the crate's
    // drop-guard cancellation rule: a profile switch is global server
    // state, not work held on behalf of one client, so it runs to
    // completion (and settles the menu) even if the clicking client
    // disconnects mid-switch.
    let client = state.gateway_client().clone();
    let push = state.push();
    let name = name.to_string();
    tokio::spawn(async move {
        run_switch(&client, &push, &name).await;
    });
}

/// How many stage markers the gateway's switch stream emits, in
/// execution order: `loading-profile`, `stopping-models`,
/// `starting-models`.
const SWITCH_STAGES: u64 = 3;

/// Runs one profile switch to its end: drives the gateway's stage
/// stream into determinate status-bar progress, refetches the profile
/// state and model catalog, and settles the menu - the remembered model
/// selected and `chat_ready` recomputed on success, the truthful
/// pre-switch state restored on failure - before pushing the idle or
/// failure status.
async fn run_switch(client: &GatewayClient, push: &Push, name: &str) {
    let outcome = drive_switch(client, push, name).await;
    // The gateway's serving state may have changed even on a failed
    // switch (its documented degraded state can lose local children),
    // so both paths refetch before the menu settles; the final
    // workbench snapshot then reads a fresh catalog and profile list.
    tokio::join!(
        refresh_profiles(client, push),
        refresh_catalog(client, push)
    );
    match outcome {
        Ok(()) => {
            push.menu().finish_switch(SwitchOutcome::Completed);
            push.push_idle();
        }
        Err(message) => {
            push.menu().finish_switch(SwitchOutcome::Failed);
            push.push_failure("Profile switch failed", message, Activity::General);
        }
    }
}

/// Posts the switch and consumes its stage stream, pushing each stage
/// marker as determinate progress, until the terminal event: `Ok` on
/// `ready`, the failure's description on everything else - a terminal
/// `error`, a buffered refusal, a transport failure, or a stream that
/// ends without a terminal event.
async fn drive_switch(client: &GatewayClient, push: &Push, name: &str) -> Result<(), String> {
    let payloads = match client.switch_profile(name).await {
        Ok(SwitchResponse::Switching { payloads, .. }) => payloads,
        Ok(SwitchResponse::Buffered(refusal)) => return Err(switch_refusal(&refusal)),
        Err(error) => return Err(error.to_string()),
    };
    let mut events = switch_events(payloads);
    while let Some(item) = events.next().await {
        match item {
            Ok(SwitchEvent::Stage { stage }) => push_stage(push, name, &stage),
            Ok(SwitchEvent::Ready { .. }) => return Ok(()),
            Ok(SwitchEvent::Error { message }) => return Err(message),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("the switch stream ended without a terminal event".to_string())
}

/// Pushes one stage marker as determinate status-bar progress - stage
/// n of [`SWITCH_STAGES`] with a label naming the stage. A stage this
/// build does not know is logged and skipped: the gateway may grow
/// stages, and a lost progress update never degrades the switch
/// (zone two).
fn push_stage(push: &Push, name: &str, stage: &str) {
    let (label, current) = match stage {
        "loading-profile" => ("Loading profile...", 1),
        "stopping-models" => ("Stopping models...", 2),
        "starting-models" => ("Starting models...", 3),
        other => {
            tracing::debug!(stage = other, "unknown switch stage; no progress pushed");
            return;
        }
    };
    push.push_progress(
        label,
        format!("switching to profile {name}"),
        current,
        SWITCH_STAGES,
        Activity::General,
    );
}

/// The failure description of a buffered switch refusal: the gateway's
/// own error message when its envelope carries one, else the status.
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
