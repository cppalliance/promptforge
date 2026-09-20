//! Pending-state read routes: `GET /admin/config-pending` and
//! `GET /admin/config-dirty`.
//!
//! The write route (`config.rs`) stages the global config as a
//! `.next` shadow beside the real file; these routes read that pending
//! state back. Profile selection is never staged: `config-pending` reports
//! the selection the real `gateway.state.toml` persists, which may differ
//! from the running profile until the next start.
//! `config-dirty` is the cheap poll: whether any shadow exists, which
//! real files carry one, and which top-level sections the pending view
//! changes. The resolution machinery lives in
//! `gateway-config`; these handlers own auth, path assembly,
//! and the wire shape.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use gateway_config::{
    Config, ProfileSelection, ProfileState, load_pending_config, pending_report,
    profile_state_path, shadow_path,
};
use serde::Serialize;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::error_chain;
use crate::error::{GatewayError, blocking};
use crate::registry::RouteInfo;

/// The `GET /admin/config-pending` reply.
#[derive(Debug, Serialize)]
pub(crate) struct PendingReply {
    /// The shadow-preferred global config document plus `active_profile`,
    /// the persisted selection.
    profile: serde_json::Value,
    /// Always `null`: the boot side has no pending view of its own.
    boot: Option<serde_json::Value>,
}

/// The `GET /admin/config-dirty` reply.
#[derive(Debug, Serialize)]
pub(crate) struct DirtyReply {
    /// Whether any shadow exists.
    dirty: bool,
    /// The real files whose shadows exist, relative to the config root,
    /// sorted.
    pending_files: Vec<String>,
    /// The top-level sections the config shadow changes.
    changed_sections: Vec<String>,
}

const PENDING: RouteInfo = RouteInfo::walled("/admin/config-pending", &[Method::GET]);
const DIRTY: RouteInfo = RouteInfo::walled("/admin/config-dirty", &[Method::GET]);

/// The pending-state read routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[PENDING, DIRTY];

/// The pending-state read routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(PENDING.path, get(admin_config_pending))
        .route(DIRTY.path, get(admin_config_dirty))
}

/// The `GET /admin/config-pending` route: bearer-authed, renders the
/// shadow-preferred global config and the persisted profile selection.
///
/// The reply keeps the existing `{"profile": ..., "boot": null}` envelope
/// for the current UI. `profile` contains the shadow-preferred global config
/// plus `active_profile`, read from the real `gateway.state.toml`: `null`
/// when no selection is persisted, otherwise the raw persisted name, even
/// when the config no longer defines it, so the UI can show a selection
/// that differs from the running profile or has gone stale. Secrets remain
/// redacted.
pub(crate) async fn admin_config_pending(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<PendingReply>, GatewayError> {
    let _publication = state.apply.lock().await;
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let running_profile = state.profile_name().await;
    let reply = blocking(move || {
        let config = load_pending_for_running(&config_path, running_profile.as_deref())?;
        let mut profile = config.to_json();
        if let Some(table) = profile.as_object_mut() {
            table.insert(
                "active_profile".to_owned(),
                persisted_selection(&config_path)?.map_or(serde_json::Value::Null, |name| {
                    serde_json::Value::String(name)
                }),
            );
        }
        Ok::<_, GatewayError>(PendingReply {
            profile,
            boot: None,
        })
    })
    .await??;
    Ok(Json(reply))
}

/// Loads the shadow-preferred config under the running selection, which
/// may have come from a command-line or environment override and
/// therefore differ from persisted state.
pub(crate) fn load_pending_for_running(
    config_path: &Path,
    running_profile: Option<&str>,
) -> Result<Config, GatewayError> {
    load_pending_config(config_path, &ProfileSelection::new(running_profile, None))
        .map_err(|error| pending_read_error(&error))
}

/// The profile name the real state file persists, `None` when the file is
/// absent. The name is not checked against any config: a stale selection
/// is still the persisted one.
fn persisted_selection(config_path: &Path) -> Result<Option<String>, GatewayError> {
    let path = profile_state_path(config_path);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(GatewayError::PendingConfig(format!(
                "read profile state {}: {source}",
                path.display()
            )));
        }
    };
    let state = ProfileState::from_toml_str(&raw).map_err(|error| pending_read_error(&error))?;
    Ok(Some(state.active_profile().to_owned()))
}

/// The `GET /admin/config-dirty` route: bearer-authed, reports the
/// pending state from shadow existence and comparison.
///
/// The reply is `{"dirty", "pending_files", "changed_sections"}`. `dirty`
/// is true when any shadow exists. `pending_files` names the real files
/// whose shadows are present - the global config and the env sibling -
/// rendered relative to the config directory with forward slashes, sorted.
/// `.env` shadows count toward `dirty` and `pending_files` only. Profile
/// selection is never pending, so neither list ever names the state file
/// or `active_profile`.
pub(crate) async fn admin_config_dirty(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<DirtyReply>, GatewayError> {
    let _publication = state.apply.lock().await;
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let reply = blocking(move || dirty_reply(&config_path)).await??;
    Ok(Json(reply))
}

/// Maps a config-crate failure on a pending read: saves validate before
/// writing, so an unresolvable pending state is a server fault (500) with
/// the full cause chain in the message.
fn pending_read_error(error: &gateway_config::ConfigError) -> GatewayError {
    GatewayError::PendingConfig(error_chain(error))
}

/// Every shadow on disk for one gateway and the sections they change.
pub(crate) struct ShadowCensus {
    /// Real files whose shadows exist, in canonical form, without
    /// duplicates.
    pub(crate) files: Vec<PathBuf>,
    /// Top-level sections whose merged value the shadows change, sorted
    /// and deduplicated.
    pub(crate) sections: Vec<String>,
}

/// Collects the config shadow and one env shadow.
pub(crate) fn shadow_census(config_path: &Path) -> Result<ShadowCensus, GatewayError> {
    let profile = pending_report(config_path).map_err(|error| pending_read_error(&error))?;
    let mut files: Vec<PathBuf> = Vec::new();
    for file in &profile.shadowed_files {
        push_unique(&mut files, file);
    }
    let sections = profile.changed_sections;
    let env = config_path.with_extension("env");
    if shadow_path(&env).is_file() {
        push_unique(&mut files, &env);
    }
    Ok(ShadowCensus { files, sections })
}

/// The directory config files render relative to.
pub(crate) fn config_root(config_path: &Path) -> Option<&Path> {
    config_path.parent()
}

/// Assembles the `GET /admin/config-dirty` body: the shadowed config file
/// plus the `.env` sibling, and the config shadow's section diff.
fn dirty_reply(config_path: &Path) -> Result<DirtyReply, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let mut pending_files: Vec<String> = census
        .files
        .iter()
        .map(|file| relative_name(file, root))
        .collect();
    pending_files.sort_unstable();
    Ok(DirtyReply {
        dirty: !pending_files.is_empty(),
        pending_files,
        changed_sections: census.sections,
    })
}

/// Appends `file` unless its canonical form is already listed. The same
/// file reaches here under different spellings (the profile chain writes
/// `profiles/../gateway.toml`, the boot path is `gateway.toml`), so the
/// list holds canonical forms.
fn push_unique(shadowed: &mut Vec<PathBuf>, file: &Path) {
    let canonical = canonical_form(file);
    if !shadowed.contains(&canonical) {
        shadowed.push(canonical);
    }
}

/// A comparable form of `path`: canonicalized when it exists, otherwise
/// its canonicalized parent plus its own name (a real `.env` may not
/// exist while its shadow does), otherwise the path as given.
pub(crate) fn canonical_form(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    path.to_path_buf()
}

/// Renders one shadowed real file for the wire: relative to `root` when
/// it sits beneath it, the full path otherwise, always with forward
/// slashes for a stable shape across platforms.
pub(crate) fn relative_name(file: &Path, root: Option<&Path>) -> String {
    let file = canonical_form(file);
    let relative = root
        .map(canonical_form)
        .and_then(|root| file.strip_prefix(&root).ok().map(Path::to_path_buf))
        .unwrap_or(file);
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

#[cfg(test)]
#[path = "config_pending-tests.rs"]
mod tests;
