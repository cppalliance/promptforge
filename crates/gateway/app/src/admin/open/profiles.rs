//! The profile routes: `GET /admin/profiles` and
//! `POST /admin/switch-profile`.

use axum::extract::State;
use axum::http::Method;
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_config::ProfileName;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::admin::config_path;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, WireJson, blocking, config_write_error};
use crate::registry::RouteInfo;

const PROFILES: RouteInfo = RouteInfo::open("/admin/profiles", &[Method::GET]);
const SWITCH_PROFILE: RouteInfo = RouteInfo::open("/admin/switch-profile", &[Method::POST]);

/// The profile routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[PROFILES, SWITCH_PROFILE];

/// The profile routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(PROFILES.path, get(admin_list_profiles))
        .route(SWITCH_PROFILE.path, post(admin_switch_profile))
}

/// The `POST /admin/switch-profile` body: a profile name, or `null` (or
/// absent) to select no profile.
#[derive(Debug, Deserialize)]
pub(crate) struct SwitchProfileRequest {
    name: Option<String>,
}

/// The `GET /admin/profiles` reply.
#[derive(Debug, Serialize)]
pub(crate) struct ProfilesReply {
    /// Every profile name the loaded catalog defines, in catalog order.
    profiles: Vec<String>,
}

/// The `POST /admin/switch-profile` reply.
#[derive(Debug, Serialize)]
pub(crate) struct SwitchProfileReply {
    /// The persisted selection, `null` when no profile is selected.
    profile: Option<String>,
    /// Whether the selection differs from the running profile, so it
    /// takes effect only at the next start.
    restart_required: bool,
}

/// Lists profile names from the loaded global catalog.
pub(crate) async fn admin_list_profiles(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<ProfilesReply>, GatewayError> {
    let config = state.config().await;
    let profiles = config
        .profiles()
        .iter()
        .map(|profile| profile.name().to_owned())
        .collect();
    Ok(Json(ProfilesReply { profiles }))
}

/// Persists the profile selection and reports whether a restart is needed.
///
/// The local runtime is fixed for the process lifetime, so a switch never
/// starts or stops anything. A defined name is written to
/// `gateway.state.toml`; `null` deletes the file, the persisted form of
/// "no profile". The reply is `{"profile": Option<String>,
/// "restart_required": bool}`, where `restart_required` is whether the
/// selection differs from the running profile. The check against the live
/// catalog and the state write run under the apply lock, so
/// `GET /admin/config-pending` never reads a half-written selection.
///
/// A malformed name fails at the `parse-name` stage; an undefined one is
/// [`GatewayError::ProfileNotFound`] naming the defined profiles; a failed
/// state write is the config-write error. Every refusal changes nothing.
pub(crate) async fn admin_switch_profile(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    WireJson(request): WireJson<SwitchProfileRequest>,
) -> Result<Json<SwitchProfileReply>, GatewayError> {
    let selected = request
        .name
        .as_deref()
        .map(ProfileName::parse)
        .transpose()
        .map_err(|e| GatewayError::switch_failed("parse-name", e))?;
    let _publication = state.apply.lock().await;
    let config_path = config_path(&state)?.to_path_buf();
    let restart_required = {
        let live = state.live.read().await;
        if let Some(name) = &selected {
            let defined: Vec<&str> = live
                .config
                .profiles()
                .iter()
                .map(gateway_config::ProfileConfig::name)
                .collect();
            if !defined.contains(&name.as_str()) {
                return Err(GatewayError::ProfileNotFound(undefined_profile_message(
                    name, &defined,
                )));
            }
        }
        selected.as_ref().map(ProfileName::as_str) != live.profile_name.as_deref()
    };
    let persisted = selected.clone();
    blocking(move || match &persisted {
        Some(name) => gateway_config::persist_profile_state(&config_path, name),
        None => gateway_config::clear_profile_state(&config_path),
    })
    .await?
    .map_err(config_write_error)?;
    Ok(Json(SwitchProfileReply {
        profile: selected.map(|name| name.as_str().to_owned()),
        restart_required,
    }))
}

/// The `profile not found` detail for a switch to a name the live catalog
/// does not define: the name, then every defined profile so the operator
/// can pick one without a second request.
fn undefined_profile_message(name: &ProfileName, defined: &[&str]) -> String {
    if defined.is_empty() {
        format!("{name} (no profiles are defined)")
    } else {
        format!("{name} (defined profiles: {})", defined.join(", "))
    }
}

#[cfg(test)]
#[path = "profiles-tests.rs"]
mod tests;
