//! Apply and revert routes for pending config shadows:
//! `POST /admin/config-apply` and `POST /admin/config-revert`.
//!
//! Apply captures the pending state under the apply lock - a census of the
//! shadows, the parsed shadow-preferred config, and every shadow's current
//! contents - then releases the lock. A change that needs no reload (an env
//! shadow, a process-owned section) is promoted inline. Anything else runs
//! as an `ApplyConfig` command on the command queue: the switch goes
//! through the same machinery as `POST /admin/switch-profile`, and the
//! captured shadows are promoted at its commit, so a failed or cancelled
//! apply promotes nothing and leaves every shadow staged for a retry.
//! Revert cancels any apply in flight, then deletes every shadow and
//! touches nothing else. Saves, the capture step, the commit, and revert
//! serialize on one mutex, so apply only captures combinations the latest
//! save validated whole. Both routes reply with plain JSON: the reload's
//! staged progress (`loading-profile`, `stopping-models`,
//! `starting-models`) streams to `GET /admin/progress` subscribers, so the
//! apply response carries the outcome and the progress stream carries the
//! stages.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use gateway_config::{
    Config, ProfileName, ProfileSelection, load_pending_config, profile_state_path, shadow_path,
    write_atomic,
};
use shared_progress::ProgressTree;
use tokio_util::sync::CancellationToken;

use crate::auth::Caller;
use crate::commands::{APPLY_CONFIG_LABEL, Command, Outcome};
use crate::config_pending::{canonical_form, config_root, relative_name, shadow_census};
use crate::config_write::{config_write_error, error_chain};
use crate::error::GatewayError;
use crate::{AppState, StatePersistence, check_auth};

/// The `POST /admin/config-apply` route: bearer-authed, applies every
/// staged shadow, reloading the selected profile when the change needs it.
///
/// The reply is plain JSON - `{"applied": [...], "reloaded": bool,
/// "restart_required": bool}` - not SSE: the reload runs as a command on
/// the queue through the same path as `POST /admin/switch-profile`, so its
/// staged progress streams to `GET /admin/progress` subscribers, and the
/// response carries the outcome. `applied` names the promoted real files
/// relative to the config root, sorted. `reloaded` is true when a config or
/// profile-state shadow applied successfully. `restart_required` is true
/// for an env shadow or a process-owned `[server]` or `[workshop]` change.
/// With no shadows on disk the reply is the clean no-op
/// `{"applied": [], "reloaded": false, "restart_required": false}`.
///
/// Nothing is promoted before the switch commits. A parse failure replies
/// 500 before any command exists; a switch failure replies
/// [`GatewayError::ApplyReloadFailed`] (500) and a cancelled command -
/// the user's cancel, a revert, or shutdown - replies
/// [`GatewayError::ApplyCancelled`] (503). In both cases every shadow is
/// still staged, so a retry of Apply re-runs the whole thing.
pub(crate) async fn admin_config_apply(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let (enqueued, applied, restart_required) = {
        // The lock spans the census, the parse, and the capture (or the
        // inline promotion), so a save cannot land between them and the
        // snapshot is one the latest save validated whole. It is released
        // before the switch runs: the queue serializes the switch itself.
        let _guard = state.apply.lock().await;
        let plan = tokio::task::spawn_blocking(move || capture_apply(&config_path))
            .await
            .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
        let snapshot = match plan {
            ApplyPlan::Inline {
                files,
                restart_required,
            } => {
                let applied = tokio::task::spawn_blocking(move || promote_captures(&files))
                    .await
                    .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
                return Ok(Json(serde_json::json!({
                    "applied": applied,
                    "reloaded": false,
                    "restart_required": restart_required,
                })));
            }
            ApplyPlan::Reload(snapshot) => snapshot,
        };
        let applied = snapshot.applied_names();
        let restart_required = snapshot.restart_required;
        let enqueued = state.commands.enqueue(Command::ApplyConfig {
            snapshot,
            token: CancellationToken::new(),
        });
        (enqueued, applied, restart_required)
    };
    let outcome = enqueued.outcome.await.unwrap_or_else(|_| {
        // The worker settles every command it begins, so a dropped sender
        // means the worker task itself died.
        Arc::new(Err(GatewayError::switch_failed(
            "queue",
            std::io::Error::other("the command queue dropped the command without settling it"),
        )))
    });
    match &*outcome {
        Ok(_) => Ok(Json(serde_json::json!({
            "applied": applied,
            "reloaded": true,
            "restart_required": restart_required,
        }))),
        Err(GatewayError::CommandCancelled(_)) => Err(GatewayError::ApplyCancelled),
        Err(error) => Err(GatewayError::ApplyReloadFailed(error_chain(error))),
    }
}

/// The `POST /admin/config-revert` route: bearer-authed, cancels any apply
/// in flight, deletes every shadow file, and touches nothing else.
///
/// The reply is `{"reverted": [...]}` naming the deleted shadow files
/// relative to the config root, sorted. The real files were never touched
/// by a save, so nothing is rewritten: deleting the shadows is the whole
/// revert. An apply cancelled here settles its route with
/// [`GatewayError::ApplyCancelled`].
pub(crate) async fn admin_config_revert(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    // A revert issued during an apply wins: cancel the apply before its
    // commit can write the snapshot over the files being reverted. The
    // commit re-checks the token under the apply lock, so an apply already
    // waiting for that lock still stops.
    state.commands.cancel_apply();
    // The same guard as apply's capture and commit: a revert must not race
    // either.
    let _guard = state.apply.lock().await;
    let config_path = crate::config_path(&state)?.to_path_buf();
    let reverted = tokio::task::spawn_blocking(move || delete_all_shadows(&config_path))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
    Ok(Json(serde_json::json!({ "reverted": reverted })))
}

/// One shadow as the Apply route captured it, ready to land in its real
/// file at the switch's commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowCapture {
    /// The real file the shadow stands in for, in canonical form.
    pub(crate) real_path: PathBuf,
    /// The real file rendered for the wire, relative to the config root.
    pub(crate) relative_name: String,
    /// The shadow's contents at capture time.
    pub(crate) contents: String,
}

/// What one reloading apply carries onto the command queue: the parsed
/// pending config, the profile it selects, and every captured shadow.
#[derive(Debug)]
pub(crate) struct ApplySnapshot {
    /// The shadow-preferred pending config, parsed and validated. Boxed so
    /// the `Command` enum stays the size of its other variants.
    pub(crate) config: Box<Config>,
    /// The profile the pending config selects.
    pub(crate) profile: ProfileName,
    /// Every shadow the census found, with its contents at capture time:
    /// the config shadow, the state shadow, and any env shadow.
    pub(crate) files: Vec<ShadowCapture>,
    /// Whether an env or process-owned setting changed.
    pub(crate) restart_required: bool,
}

impl ApplySnapshot {
    /// The captured real files rendered for the wire, sorted.
    pub(crate) fn applied_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .files
            .iter()
            .map(|file| file.relative_name.clone())
            .collect();
        names.sort_unstable();
        names
    }
}

/// What the census decided: promote inline, or reload through the queue.
enum ApplyPlan {
    /// No config or state shadow: the captures are promoted under the route's
    /// lock and nothing in the switch machinery runs.
    Inline {
        files: Vec<ShadowCapture>,
        restart_required: bool,
    },
    /// A config or state shadow: the switch runs as an `ApplyConfig` command
    /// and promotes the captures at its commit.
    Reload(ApplySnapshot),
}

/// Takes the census, parses the pending config when a reload is needed, and
/// reads every shadow's contents. Touches no real file.
fn capture_apply(config_path: &Path) -> Result<ApplyPlan, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let config_canonical = canonical_form(config_path);
    let state_canonical = canonical_form(&profile_state_path(config_path));
    let env_canonical = canonical_form(&config_path.with_extension("env"));
    let needs_reload = census
        .files
        .iter()
        .any(|file| file == &config_canonical || file == &state_canonical);
    let mut restart_required = census
        .sections
        .iter()
        .any(|section| matches!(section.as_str(), "server" | "workshop"));
    let mut files = Vec::with_capacity(census.files.len());
    for file in &census.files {
        if file == &env_canonical {
            restart_required = true;
        }
        let shadow = shadow_path(file);
        let contents = std::fs::read_to_string(&shadow)
            .map_err(|source| GatewayError::ConfigWriteIo(Box::new(source)))?;
        files.push(ShadowCapture {
            real_path: file.clone(),
            relative_name: relative_name(file, root),
            contents,
        });
    }
    if !needs_reload {
        return Ok(ApplyPlan::Inline {
            files,
            restart_required,
        });
    }
    let config = load_pending_config(config_path, &ProfileSelection::default())
        .map_err(config_write_error)?;
    let profile = config
        .active_profile()
        .ok_or(GatewayError::ActiveProfileUnavailable)?
        .name()
        .to_owned();
    let profile = ProfileName::parse(&profile)
        .map_err(|error| GatewayError::switch_failed("parse-name", error))?;
    Ok(ApplyPlan::Reload(ApplySnapshot {
        config: Box::new(config),
        profile,
        files,
        restart_required,
    }))
}

/// Lands every capture in its real file and retires the shadows it came
/// from. The caller holds the apply lock.
///
/// For each capture the real file is replaced atomically with the captured
/// contents, then the shadow that exists now is compared against them: an
/// equal shadow is deleted (promotion complete), a different one - a save
/// landed since the capture - stays in place as the next pending change,
/// and a missing one needs nothing. The two invariants this keeps exact:
/// the real files always equal what is live, and a shadow always means
/// "not yet applied". Returns the promoted real files rendered for the
/// wire, sorted.
pub(crate) fn promote_captures(captures: &[ShadowCapture]) -> Result<Vec<String>, GatewayError> {
    let mut applied = Vec::with_capacity(captures.len());
    for capture in captures {
        write_atomic(&capture.real_path, &capture.contents).map_err(config_write_error)?;
        let shadow = shadow_path(&capture.real_path);
        match std::fs::read_to_string(&shadow) {
            Ok(current) if current == capture.contents => {
                if let Err(source) = std::fs::remove_file(&shadow)
                    && source.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(GatewayError::ConfigWriteIo(Box::new(source)));
                }
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(GatewayError::ConfigWriteIo(Box::new(source))),
        }
        applied.push(capture.relative_name.clone());
    }
    applied.sort_unstable();
    Ok(applied)
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

/// The `ApplyConfig` command body: the switch to the snapshot's profile,
/// promoting the captured shadows at its commit.
pub(crate) async fn apply_config(
    state: &AppState,
    snapshot: ApplySnapshot,
    token: CancellationToken,
    tree: ProgressTree,
) -> Outcome {
    let ApplySnapshot {
        config,
        profile,
        files,
        ..
    } = snapshot;
    let result = crate::run_switch_with_config(
        state.clone(),
        profile,
        tree,
        Some(*config),
        move || StatePersistence::Promote(files),
        &token,
    )
    .await;
    match result {
        Ok(profile) => Ok(profile),
        // A partial start lands after the commit: the captures are promoted
        // and the profile is live, so it must not report as a cancellation
        // whose reply promises the shadows are still staged.
        #[cfg(feature = "local")]
        Err(error @ GatewayError::PartialStart { .. }) => Err(error),
        // Any other failure under a fired token reports as the cancellation
        // it is, however deep in the switch the stop landed.
        Err(_) if token.is_cancelled() => Err(GatewayError::CommandCancelled(
            APPLY_CONFIG_LABEL.to_owned(),
        )),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use gateway_config::{
        Config, ProfileName, ProfileSelection, ProfileState, profile_state_path, shadow_path,
        write_shadow,
    };
    use shared_progress::{EventState, ProgressEvent};
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    use crate::AppState;
    use crate::commands::Command;
    use crate::error::GatewayError;
    use crate::test_support::{AdminPaths, app_state, serve_state};

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

    /// Serves the fixture with the production queue worker running, so an
    /// apply's `ApplyConfig` command actually drains; the state comes back
    /// for tests that park the switch or read the queue.
    async fn serve_fixture(config: Config, paths: AdminPaths) -> (SocketAddr, AppState) {
        let state = app_state(config, Some(paths));
        let _worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let addr = serve_state(state.clone()).await;
        (addr, state)
    }

    async fn post(addr: SocketAddr, route: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("post sends")
    }

    async fn get_json(addr: SocketAddr, route: &str) -> serde_json::Value {
        reqwest::Client::new()
            .get(format!("http://{addr}/{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("get sends")
            .json()
            .await
            .expect("json body")
    }

    /// Stages `profile` as the pending active profile through the real
    /// save route, so the shadows are exactly what the UI would write.
    async fn save_active_profile(addr: SocketAddr, profile: &str) -> reqwest::Response {
        let mut body = get_json(addr, "admin/config").await;
        body["active_profile"] = serde_json::json!(profile);
        reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("save sends")
    }

    /// Polls `condition` with a bounded wait, for observing the worker's
    /// externally visible state transitions.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// Whether the queue's active command is the one labelled `name`.
    fn active_is(state: &AppState, name: &str) -> bool {
        state
            .commands
            .active_command()
            .is_some_and(|status| status.name == name)
    }

    /// The live profile name, as `GET /admin/status` would report it.
    async fn live_profile(state: &AppState) -> Option<String> {
        state.live.read().await.profile_name.clone()
    }

    /// How many switches the hub saw: each run of the switch machinery
    /// opens exactly one `loading-profile` leaf.
    fn switches_begun(events: &mut broadcast::Receiver<ProgressEvent>) -> usize {
        let mut count = 0;
        while let Ok(event) = events.try_recv() {
            if event.label == "loading-profile" && matches!(event.state, EventState::Begun { .. }) {
                count += 1;
            }
        }
        count
    }

    /// Asserts the apply reply is the cancellation envelope the config UI
    /// keys on.
    async fn assert_apply_cancelled(response: reqwest::Response) {
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value = response.json().await.expect("error envelope");
        assert_eq!(body["error"]["code"], "apply_cancelled");
        assert_eq!(body["error"]["type"], "server_error");
        assert_eq!(
            body["error"]["message"],
            GatewayError::ApplyCancelled.to_string()
        );
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("still staged")),
            "the message tells the user their changes survive: {body}"
        );
    }

    #[tokio::test]
    async fn apply_switches_and_persists_pending_active_profile() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, _state) = serve_fixture(config, paths).await;
        let http = reqwest::Client::new();
        let save = save_active_profile(addr, "beta").await;
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        let apply = post(addr, "admin/config-apply").await;
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
        let (addr, state) = serve_fixture(config, paths).await;
        let mut events = state.hub.subscribe();

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
        assert_eq!(
            switches_begun(&mut events),
            0,
            "the parse failure replies before any command exists"
        );
        assert!(state.commands.active_command().is_none());
        assert!(state.commands.pending_commands().is_empty());
    }

    #[tokio::test]
    async fn env_only_apply_requires_restart_without_switching() {
        let (_temp, config, paths) = fixture();
        let env_path = paths.config_path.with_extension("env");
        write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env shadow");
        let (addr, state) = serve_fixture(config, paths).await;
        let mut events = state.hub.subscribe();

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.env"]));
        assert_eq!(reply["reloaded"], false);
        assert_eq!(reply["restart_required"], true);
        assert_eq!(
            std::fs::read_to_string(&env_path).expect("read promoted env"),
            "HF_TOKEN=pending\n"
        );
        assert!(
            !shadow_path(&env_path).exists(),
            "the promoted shadow is retired"
        );
        assert_eq!(
            switches_begun(&mut events),
            0,
            "the no-reload path promotes inline without a command"
        );
    }

    #[tokio::test]
    async fn server_key_change_waits_for_restart() {
        let (_temp, config, paths) = fixture();
        let (addr, _state) = serve_fixture(config, paths).await;
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
        let (addr, _state) = serve_fixture(config, paths).await;

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

    /// Two applies in flight at once share one command: the second attaches
    /// to the first through the debounce, both replies carry the same
    /// `applied` list, and the switch machinery runs exactly once.
    #[tokio::test]
    async fn concurrent_applies_promote_pending_state_once() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let (addr, state) = serve_fixture(config, paths).await;
        let mut events = state.hub.subscribe();
        let save = save_active_profile(addr, "beta").await;
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        // A registered request parks the first apply's switch in its drain,
        // so the second apply provably arrives while the first is active.
        let held = state.in_flight.register();
        let first = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the first apply to go active", || {
            active_is(&state, "apply-config")
        })
        .await;
        let second = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the second apply to attach to the first", || {
            state.commands.active_waiters() == 2
        })
        .await;
        assert!(
            state.commands.pending_commands().is_empty(),
            "the second apply attached to the active one instead of queueing"
        );
        drop(held);

        let first = first.await.expect("first apply task");
        let second = second.await.expect("second apply task");
        assert_eq!(first.status(), reqwest::StatusCode::OK);
        assert_eq!(second.status(), reqwest::StatusCode::OK);
        let first: serde_json::Value = first.json().await.expect("first body");
        let second: serde_json::Value = second.json().await.expect("second body");
        let expected = serde_json::json!(["gateway.state.toml", "gateway.toml"]);
        assert_eq!(first["applied"], expected);
        assert_eq!(
            second["applied"], expected,
            "both replies report the shared outcome"
        );
        assert_eq!(first["reloaded"], true);
        assert_eq!(second["reloaded"], true);
        assert_eq!(
            switches_begun(&mut events),
            1,
            "the attached duplicate never runs a second switch"
        );
        assert!(!shadow_path(&config_path).exists());
        assert!(!shadow_path(&state_path).exists());
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("read state"),
            "active_profile = \"beta\"\n"
        );
        assert_eq!(live_profile(&state).await.as_deref(), Some("beta"));
    }

    /// The deadlock this run fixes: an apply requested while a `LoadProfile`
    /// is active used to wait on the apply lock the switch held for its whole
    /// download. Now the apply supersedes the switch - which settles as
    /// cancelled while the request that parked it is still held - and then
    /// completes.
    #[tokio::test]
    async fn an_apply_during_an_active_load_profile_supersedes_it_and_completes() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let (addr, state) = serve_fixture(config, paths).await;
        write_shadow(&state_path, "active_profile = \"beta\"\n").expect("stage state");

        let held = state.in_flight.register();
        let switch = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            true,
            CancellationToken::new(),
        ));
        wait_until("the switch to go active", || {
            active_is(&state, "load-profile: alpha")
        })
        .await;

        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        let outcome = tokio::time::timeout(Duration::from_secs(10), switch.outcome)
            .await
            .expect("the superseded switch settles while the request is still held")
            .expect("the switch settles");
        assert!(
            matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
            "the apply cancels the active switch: {outcome:?}"
        );
        wait_until("the apply to go active", || {
            active_is(&state, "apply-config")
        })
        .await;
        drop(held);

        let response = apply.await.expect("apply task");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["applied"], serde_json::json!(["gateway.state.toml"]));
        assert!(!shadow_path(&state_path).exists());
        assert_eq!(live_profile(&state).await.as_deref(), Some("beta"));
    }

    /// A cancelled apply promotes nothing: every shadow stays on disk with
    /// its contents, the dirty report is unchanged, the reply is the
    /// cancellation envelope, and a retry applies the same changes.
    #[tokio::test]
    async fn a_cancelled_apply_leaves_every_shadow_staged_and_a_retry_succeeds() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        let original_state = std::fs::read_to_string(&state_path).expect("read state");
        let (addr, state) = serve_fixture(config, paths).await;
        write_shadow(&config_path, &original_config).expect("stage config");
        write_shadow(&state_path, "active_profile = \"beta\"\n").expect("stage state");
        let dirty_before = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty_before["dirty"], true);

        let held = state.in_flight.register();
        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the apply to go active", || {
            active_is(&state, "apply-config")
        })
        .await;
        assert!(state.commands.cancel_active());

        assert_apply_cancelled(apply.await.expect("apply task")).await;
        assert_eq!(
            std::fs::read_to_string(shadow_path(&config_path)).expect("config shadow"),
            original_config,
            "the config shadow is still staged"
        );
        assert_eq!(
            std::fs::read_to_string(shadow_path(&state_path)).expect("state shadow"),
            "active_profile = \"beta\"\n",
            "the state shadow is still staged"
        );
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("re-read state"),
            original_state,
            "nothing was promoted"
        );
        assert_eq!(
            get_json(addr, "admin/config-dirty").await,
            dirty_before,
            "the dirty report is unchanged"
        );
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
        drop(held);

        let retry = post(addr, "admin/config-apply").await;
        assert_eq!(retry.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = retry.json().await.expect("retry body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(
            reply["applied"],
            serde_json::json!(["gateway.state.toml", "gateway.toml"])
        );
        assert!(!shadow_path(&config_path).exists());
        assert!(!shadow_path(&state_path).exists());
        assert_eq!(live_profile(&state).await.as_deref(), Some("beta"));
    }

    /// A save that lands mid-apply neither blocks nor is lost: the snapshot's
    /// contents land in the real file, and the newer shadow stays pending as
    /// the next change.
    #[tokio::test]
    async fn a_save_landing_mid_apply_stays_pending_while_the_snapshot_lands() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let (addr, state) = serve_fixture(config, paths).await;
        write_shadow(&state_path, "active_profile = \"beta\"\n").expect("stage state");

        let held = state.in_flight.register();
        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the apply to go active", || {
            active_is(&state, "apply-config")
        })
        .await;
        let save =
            tokio::time::timeout(Duration::from_secs(10), save_active_profile(addr, "alpha"))
                .await
                .expect("the save completes while the apply is active");
        assert_eq!(save.status(), reqwest::StatusCode::OK);
        drop(held);

        let response = apply.await.expect("apply task");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.state.toml"]));
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("read state"),
            "active_profile = \"beta\"\n",
            "the snapshot's contents landed in the real file"
        );
        assert_eq!(
            std::fs::read_to_string(shadow_path(&state_path)).expect("state shadow"),
            "active_profile = \"alpha\"\n",
            "the newer save stays pending instead of being deleted"
        );
        assert!(
            shadow_path(&config_path).is_file(),
            "the save's config shadow, absent from the snapshot, is untouched"
        );
        assert_eq!(live_profile(&state).await.as_deref(), Some("beta"));
        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(
            dirty["pending_files"],
            serde_json::json!(["gateway.state.toml", "gateway.toml"])
        );
    }

    /// A revert during an active apply wins: the apply settles as cancelled,
    /// its commit writes nothing, and the shadows are gone.
    #[tokio::test]
    async fn a_revert_during_an_active_apply_cancels_it_and_the_commit_writes_nothing() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let state_path = profile_state_path(&config_path);
        let original_state = std::fs::read_to_string(&state_path).expect("read state");
        let (addr, state) = serve_fixture(config, paths).await;
        write_shadow(&state_path, "active_profile = \"beta\"\n").expect("stage state");

        let held = state.in_flight.register();
        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the apply to go active", || {
            active_is(&state, "apply-config")
        })
        .await;

        let revert = post(addr, "admin/config-revert").await;
        assert_eq!(revert.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = revert.json().await.expect("revert body");
        assert_eq!(
            reply["reverted"],
            serde_json::json!(["gateway.state.toml.next"])
        );

        assert_apply_cancelled(apply.await.expect("apply task")).await;
        drop(held);
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("re-read state"),
            original_state,
            "the cancelled apply's commit wrote nothing"
        );
        assert!(!shadow_path(&state_path).exists());
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
        wait_until("the queue to go idle", || {
            state.commands.active_command().is_none()
        })
        .await;
    }
}
