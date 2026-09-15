//! Gateway profile and model-catalog refresh after reachability.

use workshop_protocol::is_chat_capable;
use workshop_registry::Push;

use crate::gateway::GatewayClient;

/// Re-fetches the gateway's model catalog and pushes it to every session.
///
/// A failed, declined, or malformed catalog is logged and skipped rather
/// than pushed: pushing a bad snapshot would clear pickers that still hold
/// a usable list.
pub async fn refresh_catalog(client: &GatewayClient, push: &Push) -> bool {
    let response = match client.list_models().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "catalog refresh failed");
            return false;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "catalog refresh was declined");
        return false;
    }
    let body: serde_json::Value = match serde_json::from_slice(&response.body) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(%error, "catalog refresh was not JSON");
            return false;
        }
    };
    let Some(models) = body.get("data").and_then(serde_json::Value::as_array) else {
        tracing::warn!("catalog refresh carried no data array");
        return false;
    };
    let selectable = models.iter().any(is_chat_capable);
    push.push_models_catalog(models.clone());
    selectable
}

/// The decoded body of `GET /admin/profiles`.
#[derive(serde::Deserialize)]
struct ProfileList {
    /// Every profile name the gateway can serve, in gateway order.
    profiles: Vec<String>,
}

/// The decoded body of `GET /admin/status`, reduced to the one field the
/// menu needs.
#[derive(serde::Deserialize)]
struct ProfileStatus {
    /// The profile the gateway is serving.
    #[serde(default)]
    profile: Option<String>,
}

/// Fetches the gateway's profile list and active profile and publishes
/// them into the workbench snapshot.
///
/// A gateway without profile support is a state, not an error: a failed,
/// declined, or malformed answer degrades that half to empty, so the menu
/// shows no profiles rather than stale names.
pub async fn refresh_profiles(client: &GatewayClient, push: &Push) -> bool {
    let (profiles, active) = tokio::join!(fetch_profile_list(client), fetch_active_profile(client));
    let ready = profiles.as_ref().is_some_and(|profiles| {
        !profiles.is_empty()
            && active
                .as_ref()
                .is_some_and(|active| profiles.contains(active))
    });
    push.menu()
        .set_profiles(profiles.unwrap_or_default(), active);
    ready
}

/// The gateway's profile names, or `None` on a failed response.
async fn fetch_profile_list(client: &GatewayClient) -> Option<Vec<String>> {
    let response = match client.list_profiles().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "profile list fetch failed");
            return None;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "profile list was declined");
        return None;
    }
    match serde_json::from_slice::<ProfileList>(&response.body) {
        Ok(list) => Some(list.profiles),
        Err(error) => {
            tracing::warn!(%error, "profile list was not the expected JSON");
            None
        }
    }
}

/// The active profile name, or `None` on a failed response.
async fn fetch_active_profile(client: &GatewayClient) -> Option<String> {
    let response = match client.profile_status().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "profile status fetch failed");
            return None;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "profile status was declined");
        return None;
    }
    match serde_json::from_slice::<ProfileStatus>(&response.body) {
        Ok(status) => status.profile,
        Err(error) => {
            tracing::warn!(%error, "profile status was not the expected JSON");
            None
        }
    }
}
