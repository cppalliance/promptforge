//! Apply and revert routes for pending config shadows:
//! `POST /admin/config-apply` and `POST /admin/config-revert`.
//!
//! Apply parses the shadow-preferred global config, promotes its config
//! shadow, switches through the same machinery as
//! `POST /admin/switch-profile`, and commits profile state inside the switch
//! lock. A promoted env shadow reports `restart_required`.
//! Revert
//! deletes every shadow and touches nothing else. Both routes serialize on
//! one mutex - shared with the shadow-writing `PUT` saves, so apply only
//! promotes combinations the latest save validated whole and a second
//! apply can never race a half-promoted first - and
//! both reply with plain JSON: the reload's staged progress
//! (`loading-profile`, `stopping-models`, `starting-models`) streams to
//! `GET /admin/progress` subscribers, so the apply response carries the
//! outcome and the progress stream carries the stages.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use gateway_config::{
    Config, ProfileName, ProfileSelection, load_pending_config, profile_state_path, promote_shadow,
    shadow_path,
};

use crate::config_pending::{canonical_form, config_root, relative_name, shadow_census};
use crate::config_write::{config_write_error, error_chain};
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `POST /admin/config-apply` route: bearer-authed, promotes every
/// shadow to its real file, then applies the selected profile when needed.
///
/// The reply is plain JSON - `{"applied": [...], "reloaded": bool,
/// "restart_required": bool}` - not SSE: the reload runs through the same
/// path as `POST /admin/switch-profile`, so its staged progress streams to
/// `GET /admin/progress` subscribers, and the response carries the
/// outcome. `applied` names the promoted real files relative to the config
/// root, sorted. `reloaded` is true when a config or profile-state shadow
/// applied successfully. `restart_required` is true for an env shadow or a
/// process-owned `[server]` or `[workshop]` change.
/// With no shadows on disk the reply is the clean no-op
/// `{"applied": [], "reloaded": false, "restart_required": false}`.
///
/// Config promotion happens before switching, while profile-state promotion
/// is serialized with successful activation. The error reply
/// ([`GatewayError::ApplyReloadFailed`], 500) tells the caller to inspect
/// status and retry Apply.
pub(crate) async fn admin_config_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // One apply or revert at a time: the guard spans enumeration,
    // promotion, and the reload, so a racing second request observes
    // either no shadows or the fully applied state, never a half-promoted
    // one.
    let _guard = state.apply.lock().await;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let apply_path = config_path.clone();
    let promotion = tokio::task::spawn_blocking(move || prepare_apply(&apply_path))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
    let mut reloaded = false;
    let state_path = promotion.state.clone();
    if let Some(config) = promotion.config {
        apply_config(&state, config, state_path.clone())
            .await
            .map_err(|error| GatewayError::ApplyReloadFailed(error_chain(&error)))?;
        reloaded = true;
    }
    let mut applied = promotion.applied;
    if let Some(state_path) = state_path {
        let rendered = relative_name(&state_path, config_root(&config_path));
        applied.push(rendered);
        applied.sort_unstable();
    }
    Ok(Json(serde_json::json!({
        "applied": applied,
        "reloaded": reloaded,
        "restart_required": promotion.restart_required,
    })))
}

/// The `POST /admin/config-revert` route: bearer-authed, deletes every
/// shadow file and touches nothing else.
///
/// The reply is `{"reverted": [...]}` naming the deleted shadow files
/// relative to the config root, sorted. The real files were never touched
/// by a save, so nothing is rewritten: deleting the shadows is the whole
/// revert.
pub(crate) async fn admin_config_revert(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // The same guard as apply: a revert must not race an in-flight apply's
    // enumeration and promotion.
    let _guard = state.apply.lock().await;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let reverted = tokio::task::spawn_blocking(move || delete_all_shadows(&config_path))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
    Ok(Json(serde_json::json!({ "reverted": reverted })))
}

/// What one apply promoted: the rendered file names and the classification
/// deciding the follow-up (reload, restart banner, or neither).
struct Promotion {
    /// Promoted real files relative to the config root, sorted.
    applied: Vec<String>,
    /// Parsed shadow-preferred config selected for this apply.
    config: Option<Config>,
    /// State file whose shadow is promoted only after the switch succeeds.
    state: Option<PathBuf>,
    /// Whether an env or process-owned setting changed.
    restart_required: bool,
}

/// Parses pending state, promotes config and env shadows, and defers state.
fn prepare_apply(config_path: &Path) -> Result<Promotion, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let config_canonical = canonical_form(config_path);
    let state_path = profile_state_path(config_path);
    let state_canonical = canonical_form(&state_path);
    let env_canonical = canonical_form(&config_path.with_extension("env"));
    let needs_reload = census
        .files
        .iter()
        .any(|file| file == &config_canonical || file == &state_canonical);
    let restart_required = census
        .sections
        .iter()
        .any(|section| matches!(section.as_str(), "server" | "workshop"));
    let config = needs_reload
        .then(|| load_pending_config(config_path, &ProfileSelection::default()))
        .transpose()
        .map_err(config_write_error)?;
    let mut promotion = Promotion {
        applied: Vec::new(),
        config,
        state: None,
        restart_required,
    };
    for file in &census.files {
        if file == &state_canonical {
            promotion.state = Some(state_path.clone());
            continue;
        }
        promote_shadow(file).map_err(config_write_error)?;
        if file == &env_canonical {
            promotion.restart_required = true;
        }
        promotion.applied.push(relative_name(file, root));
    }
    promotion.applied.sort_unstable();
    Ok(promotion)
}

/// Deletes every shadow the census finds, returning the deleted shadow
/// files relative to the config root, sorted.
fn delete_all_shadows(config_path: &Path) -> Result<Vec<String>, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let mut reverted: Vec<String> = Vec::with_capacity(census.files.len());
    for file in &census.files {
        let shadow: PathBuf = shadow_path(file);
        std::fs::remove_file(&shadow)
            .map_err(|source| GatewayError::ConfigWriteIo(Box::new(source)))?;
        reverted.push(relative_name(&shadow, root));
    }
    reverted.sort_unstable();
    Ok(reverted)
}

/// Applies a parsed pending catalog through the switch machinery.
async fn apply_config(
    state: &AppState,
    config: Config,
    state_path: Option<PathBuf>,
) -> Result<(), GatewayError> {
    let profile = config
        .active_profile()
        .ok_or(GatewayError::ActiveProfileUnavailable)?
        .name()
        .to_owned();
    let name = ProfileName::parse(&profile)
        .map_err(|error| GatewayError::switch_failed("parse-name", error))?;
    let tree = state.hub.operation();
    let persistence = state_path.map_or(
        crate::StatePersistence::None,
        crate::StatePersistence::Promote,
    );
    // Apply is not a queue command: it already holds the apply lock, and its
    // caller waits on the reply, so it runs the switch uncancellable.
    crate::run_switch_with_config(
        state.clone(),
        name,
        tree,
        Some(config),
        || persistence,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use gateway_config::{
        Config, ProfileSelection, ProfileState, profile_state_path, shadow_path, write_shadow,
    };

    use crate::test_support::{AdminPaths, serve_with_paths};

    const CONFIG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[model]]
name = "beta-model"
description = "beta"
context = 1024
upstream = "beta"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = ["alpha-model"]

[[profile]]
name = "beta"
models = ["beta-model"]
"#;

    fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, CONFIG).expect("write config");
        std::fs::write(
            profile_state_path(&config_path),
            "active_profile = \"alpha\"\n",
        )
        .expect("write state");
        let config = Config::load(&config_path, &ProfileSelection::default()).expect("load config");
        let paths = AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "alpha".to_owned(),
            config_path,
        };
        (temp, config, paths)
    }

    async fn post(addr: std::net::SocketAddr, route: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("post sends")
    }

    #[tokio::test]
    async fn apply_switches_and_persists_pending_active_profile() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        let mut body: serde_json::Value = http
            .get(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("get sends")
            .json()
            .await
            .expect("config json");
        body["active_profile"] = serde_json::json!("beta");
        let save = http
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("save sends");
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        let apply = http
            .post(format!("http://{addr}/admin/config-apply"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("apply sends");
        assert_eq!(apply.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = apply.json().await.expect("apply json");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(
            reply["applied"],
            serde_json::json!(["gateway.state.toml", "gateway.toml"])
        );

        let state_path = profile_state_path(&config_path);
        let state =
            ProfileState::from_toml_str(&std::fs::read_to_string(&state_path).expect("read state"))
                .expect("parse state");
        assert_eq!(state.active_profile(), "beta");
        assert!(!shadow_path(&config_path).exists());
        assert!(!shadow_path(&state_path).exists());
        let applied_config = std::fs::read_to_string(&config_path).expect("read applied config");
        assert!(
            !applied_config.contains("active_profile"),
            "active selection stays in the sibling state file"
        );
        let status: serde_json::Value = http
            .get(format!("http://{addr}/admin/status"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("status sends")
            .json()
            .await
            .expect("status json");
        assert_eq!(status["profile"], "beta");
        assert_eq!(status["models"], serde_json::json!(["beta-model"]));
    }

    #[tokio::test]
    async fn invalid_pending_config_is_never_promoted() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        let original_state = std::fs::read_to_string(&state_path).expect("read state");
        write_shadow(&config_path, "not valid TOML [[[").expect("stage tampered shadow");
        let addr = serve_with_paths(config, paths).await;

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config
        );
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("re-read state"),
            original_state
        );
        assert!(
            shadow_path(&config_path).is_file(),
            "the rejected shadow remains available for correction or revert"
        );
    }

    #[tokio::test]
    async fn env_only_apply_requires_restart_without_switching() {
        let (_temp, config, paths) = fixture();
        let env_path = paths.config_path.with_extension("env");
        write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env shadow");
        let addr = serve_with_paths(config, paths).await;

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.env"]));
        assert_eq!(reply["reloaded"], false);
        assert_eq!(reply["restart_required"], true);
        assert_eq!(
            std::fs::read_to_string(env_path).expect("read promoted env"),
            "HF_TOKEN=pending\n"
        );
    }

    #[tokio::test]
    async fn server_key_change_waits_for_restart() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        let mut body: serde_json::Value = http
            .get(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("config request sends")
            .json()
            .await
            .expect("config body");
        body["server"]["api_key"] = serde_json::json!("next-token");
        let save = http
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("save sends");
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["restart_required"], true);
        assert_eq!(
            http.get(format!("http://{addr}/v1/models"))
                .bearer_auth("test-token")
                .send()
                .await
                .expect("old token request sends")
                .status(),
            reqwest::StatusCode::OK
        );
        assert_eq!(
            http.get(format!("http://{addr}/v1/models"))
                .bearer_auth("next-token")
                .send()
                .await
                .expect("new token request sends")
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn revert_removes_all_shadows_without_touching_real_files() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let env_path = config_path.with_extension("env");
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        let original_state = std::fs::read_to_string(&state_path).expect("read state");
        write_shadow(&config_path, &original_config).expect("stage config");
        write_shadow(&state_path, "active_profile = \"beta\"\n").expect("stage state");
        write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env");
        let addr = serve_with_paths(config, paths).await;

        let response = post(addr, "admin/config-revert").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("revert body");
        assert_eq!(
            reply["reverted"],
            serde_json::json!([
                "gateway.env.next",
                "gateway.state.toml.next",
                "gateway.toml.next"
            ])
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config
        );
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("re-read state"),
            original_state
        );
        assert!(!env_path.exists());
    }

    #[tokio::test]
    async fn concurrent_applies_promote_pending_state_once() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        let mut body: serde_json::Value = http
            .get(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("config request sends")
            .json()
            .await
            .expect("config body");
        body["active_profile"] = serde_json::json!("beta");
        let save = http
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("save sends");
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        let (first, second) = tokio::join!(
            post(addr, "admin/config-apply"),
            post(addr, "admin/config-apply")
        );

        assert_eq!(first.status(), reqwest::StatusCode::OK);
        assert_eq!(second.status(), reqwest::StatusCode::OK);
        let first: serde_json::Value = first.json().await.expect("first body");
        let second: serde_json::Value = second.json().await.expect("second body");
        let mut applied = [first["applied"].clone(), second["applied"].clone()];
        applied.sort_unstable_by_key(|value| value.as_array().map_or(0, Vec::len));
        assert_eq!(
            applied,
            [
                serde_json::json!([]),
                serde_json::json!(["gateway.state.toml", "gateway.toml"])
            ]
        );
    }
}
