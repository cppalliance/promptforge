//! Pending-state read routes: `GET /admin/config-pending` and
//! `GET /admin/config-dirty`.
//!
//! The write route (`config_write.rs`) stages the global config and profile
//! state as `.next` shadows beside the real files; these routes read that
//! pending state back.
//! `config-dirty` is the cheap poll: whether any shadow exists, which
//! real files carry one, and which top-level sections the pending view
//! changes. The resolution machinery lives in
//! `promptforge-gateway-config`; these handlers own auth, path assembly,
//! and the wire shape.

use std::path::{Path, PathBuf};

use crate::error::GatewayError;
use crate::{AppState, check_auth};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use promptforge_gateway_config::{
    Config, ProfileSelection, load_pending_config, pending_report, profile_state_path, shadow_path,
};

/// The `GET /admin/config-pending` route: bearer-authed, renders the
/// shadow-preferred global config and profile state.
///
/// The reply keeps the existing `{"profile": ..., "boot": null}` envelope
/// for the current UI. `profile` contains the shadow-preferred global config
/// plus `active_profile`; secrets remain redacted.
pub(crate) async fn admin_config_pending(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let running_profile = state.live.read().await.profile_name.clone();
    let reply = tokio::task::spawn_blocking(move || {
        let config = load_pending_for_running(&config_path, running_profile.as_deref())?;
        let mut profile = config.to_json();
        if let Some(table) = profile.as_object_mut()
            && let Some(active) = config.active_profile()
        {
            table.insert(
                "active_profile".to_owned(),
                serde_json::Value::String(active.name().to_owned()),
            );
        }
        Ok::<_, GatewayError>(serde_json::json!({
            "profile": profile,
            "boot": null,
        }))
    })
    .await
    .map_err(|join| GatewayError::PendingConfig(join.to_string()))??;
    Ok(Json(reply))
}

/// Loads the shadow-preferred config with the same active-selection rule as
/// `GET /admin/config-pending`.
pub(crate) fn load_pending_for_running(
    config_path: &Path,
    running_profile: Option<&str>,
) -> Result<Config, GatewayError> {
    // A pending state shadow is an explicit edit and wins. Otherwise use
    // the running selection, which may have come from a command-line or
    // environment override and therefore differ from persisted state.
    let selection = if shadow_path(&profile_state_path(config_path)).is_file() {
        ProfileSelection::default()
    } else {
        ProfileSelection::new(running_profile, None)
    };
    load_pending_config(config_path, &selection).map_err(|error| pending_read_error(&error))
}

/// The `GET /admin/config-dirty` route: bearer-authed, reports the
/// pending state from shadow existence and comparison.
///
/// The reply is `{"dirty", "pending_files", "changed_sections"}`. `dirty`
/// is true when any shadow exists. `pending_files` names the real files
/// whose shadows are present - the global config, profile state, and env
/// sibling - rendered relative to the config directory with forward slashes,
/// sorted. `.env` shadows count toward `dirty` and `pending_files` only.
pub(crate) async fn admin_config_dirty(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let reply = tokio::task::spawn_blocking(move || dirty_reply(&config_path))
        .await
        .map_err(|join| GatewayError::PendingConfig(join.to_string()))??;
    Ok(Json(reply))
}

/// Maps a config-crate failure on a pending read: saves validate before
/// writing, so an unresolvable pending state is a server fault (500) with
/// the full cause chain in the message.
fn pending_read_error(error: &promptforge_gateway_config::ConfigError) -> GatewayError {
    GatewayError::PendingConfig(crate::config_write::error_chain(error))
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

/// Collects the config, profile-state, and one env shadow.
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

/// Assembles the `GET /admin/config-dirty` body: shadowed files from the
/// profile and boot chains plus the `.env` siblings, section diffs from
/// both chain comparisons.
fn dirty_reply(config_path: &Path) -> Result<serde_json::Value, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let mut pending_files: Vec<String> = census
        .files
        .iter()
        .map(|file| relative_name(file, root))
        .collect();
    pending_files.sort_unstable();
    Ok(serde_json::json!({
        "dirty": !pending_files.is_empty(),
        "pending_files": pending_files,
        "changed_sections": census.sections,
    }))
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
mod tests {
    use promptforge_gateway_config::{
        Config, ProfileSelection, profile_state_path, shadow_path, write_shadow,
    };

    use super::*;
    use crate::test_support::{AdminPaths, serve_with_paths};

    const PROFILE_CONFIG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[profile]]
name = "alpha"
models = []

[[profile]]
name = "beta"
models = []
"#;

    #[test]
    fn dirty_reply_includes_config_and_profile_state_shadows() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config = temp.path().join("gateway.toml");
        std::fs::write(
            &config,
            "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n",
        )
        .expect("write config");
        let state = profile_state_path(&config);
        std::fs::write(&state, "active_profile = \"alpha\"\n").expect("write state");
        write_shadow(
            &config,
            "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"changed\"\n",
        )
        .expect("write config shadow");
        write_shadow(&state, "active_profile = \"beta\"\n").expect("write state shadow");

        let reply = dirty_reply(&config).expect("dirty reply");

        assert_eq!(reply["dirty"], true);
        assert_eq!(
            reply["pending_files"],
            serde_json::json!(["gateway.state.toml", "gateway.toml"])
        );
        assert_eq!(
            reply["changed_sections"],
            serde_json::json!(["active_profile", "server"])
        );
        assert!(shadow_path(&config).is_file());
    }

    #[tokio::test]
    async fn pending_view_uses_the_running_override_without_state() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, PROFILE_CONFIG).expect("write config");
        let config = Config::load(&config_path, &ProfileSelection::new(Some("beta"), None))
            .expect("load command-line override");
        let addr = serve_with_paths(
            config,
            AdminPaths {
                fixture_dir: temp.path().to_path_buf(),
                active: "beta".to_owned(),
                config_path,
            },
        )
        .await;

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/config-pending"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("pending request sends");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("pending body is JSON");
        assert_eq!(body["profile"]["active_profile"], "beta");
    }
}
