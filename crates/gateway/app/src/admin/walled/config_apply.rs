//! Apply and revert routes for pending config shadows:
//! `POST /admin/config-apply` and `POST /admin/config-revert`.
//!
//! Apply captures the pending state under the apply lock - a census of the
//! shadows, the parsed shadow-preferred config, and every shadow's current
//! contents - then releases the lock. A change that needs no reload (an env
//! shadow alone) is promoted inline. A config shadow runs as an
//! `ApplyConfig` command on the command queue: the command rebuilds the
//! remote routing table from the pending config, merges the running local
//! models under it, promotes the captured shadows under the apply lock, and
//! swaps the live routing, config, and web-search state in one write. A
//! failed or cancelled apply promotes nothing and leaves every shadow staged
//! for a retry. Sections the process reads once at boot (`[server]`,
//! `[workshop]`, `[[profile]]`, `[[local_model]]`, `[[stt_model]]`, `[stt]`)
//! and env shadows promote the same way but report `restart_required`: the
//! local runtime is fixed for the process lifetime. Revert cancels any apply
//! in flight, then deletes every shadow and touches nothing else. Saves, the
//! capture step, the commit, and revert serialize on one mutex, so apply only
//! captures combinations the latest save validated whole. Both routes reply
//! with plain JSON; the reload's `"Applying configuration"` text reaches
//! `GET /admin/progress` subscribers through the command's activity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use gateway_config::{Config, ProfileSelection, load_pending_config, shadow_path, write_atomic};
use gateway_progress::Activity;
#[cfg(feature = "web-search")]
use gateway_web_search::WebSearchState;
use tokio_util::sync::CancellationToken;

use super::config::{config_write_error, error_chain};
use super::config_pending::{canonical_form, config_root, relative_name, shadow_census};
use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::commands::{APPLY_CONFIG_LABEL, Command, Outcome};
use crate::error::{GatewayError, blocking};
use crate::registry::RouteInfo;
use crate::routing::Routing;

const APPLY: RouteInfo = RouteInfo::walled("/admin/config-apply", &[Method::POST]);
const REVERT: RouteInfo = RouteInfo::walled("/admin/config-revert", &[Method::POST]);

/// The apply and revert routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[APPLY, REVERT];

/// The apply and revert routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(APPLY.path, post(admin_config_apply))
        .route(REVERT.path, post(admin_config_revert))
}

/// Top-level sections the process reads once at boot. A change to one of
/// them promotes to disk but takes effect at the next start, so the apply
/// reports `restart_required`.
const RESTART_SECTIONS: [&str; 6] = [
    "server",
    "workshop",
    "profile",
    "local_model",
    "stt_model",
    "stt",
];

/// The `POST /admin/config-apply` route: bearer-authed, applies every
/// staged shadow, reloading the remote routing table when the change needs
/// it.
///
/// The reply is plain JSON - `{"applied": [...], "reloaded": bool,
/// "restart_required": bool}` - not SSE: the reload runs as a command on
/// the queue, so its `"Applying configuration"` text reaches
/// `GET /admin/progress` subscribers, and the response carries the outcome.
/// `applied` names the promoted real files relative to the config root,
/// sorted. `reloaded` is true when a config shadow applied successfully.
/// `restart_required` is true for an env shadow or a change to a section
/// the process reads once at boot: `[server]`, `[workshop]`, `[[profile]]`,
/// `[[local_model]]`, `[[stt_model]]`, or `[stt]`. With no shadows on disk
/// the reply is the clean no-op
/// `{"applied": [], "reloaded": false, "restart_required": false}`.
///
/// Nothing is promoted before the command commits. A parse failure replies
/// 500 before any command exists; a reload failure replies
/// [`GatewayError::ApplyReloadFailed`] (500) and a cancelled command -
/// the user's cancel, a revert, or shutdown - replies
/// [`GatewayError::ApplyCancelled`] (503). In both cases every shadow is
/// still staged, so a retry of Apply re-runs the whole thing.
pub(crate) async fn admin_config_apply(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let (enqueued, applied, restart_required) = {
        // The lock spans the census, the parse, and the capture (or the
        // inline promotion), so a save cannot land between them and the
        // snapshot is one the latest save validated whole. It is released
        // before the command runs: the queue serializes the reload itself.
        let _guard = state.apply.lock().await;
        let plan = blocking(move || capture_apply(&config_path)).await??;
        let snapshot = match plan {
            ApplyPlan::Inline {
                files,
                restart_required,
            } => {
                let applied = blocking(move || promote_captures(&files)).await??;
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
    _caller: LoopbackCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    // A revert issued during an apply wins: cancel the apply before its
    // commit can write the snapshot over the files being reverted. The
    // commit re-checks the token under the apply lock, so an apply already
    // waiting for that lock still stops.
    state.commands.cancel_apply();
    // The same guard as apply's capture and commit: a revert must not race
    // either.
    let _guard = state.apply.lock().await;
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let reverted = blocking(move || delete_all_shadows(&config_path)).await??;
    Ok(Json(serde_json::json!({ "reverted": reverted })))
}

/// One shadow as the Apply route captured it, ready to land in its real
/// file at the command's commit.
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
/// pending config and every captured shadow.
#[derive(Debug)]
pub(crate) struct ApplySnapshot {
    /// The shadow-preferred pending config, parsed and validated, with no
    /// profile selected (`active_profile()` is `None` and the local and
    /// speech-to-text subsets are empty): the apply swaps the remote
    /// catalog and never the local runtime. Boxed so the `Command` enum
    /// stays the size of its other variants.
    pub(crate) config: Box<Config>,
    /// Every shadow the census found, with its contents at capture time:
    /// the config shadow and any env shadow.
    pub(crate) files: Vec<ShadowCapture>,
    /// Whether an env or boot-read setting changed.
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
    /// No config shadow: the captures are promoted under the route's lock
    /// and no command runs.
    Inline {
        files: Vec<ShadowCapture>,
        restart_required: bool,
    },
    /// A config shadow: the reload runs as an `ApplyConfig` command and
    /// promotes the captures at its commit.
    Reload(ApplySnapshot),
}

/// Takes the census, parses the pending config when a reload is needed, and
/// reads every shadow's contents. Touches no real file.
fn capture_apply(config_path: &Path) -> Result<ApplyPlan, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let config_canonical = canonical_form(config_path);
    let env_canonical = canonical_form(&config_path.with_extension("env"));
    let needs_reload = census.files.iter().any(|file| file == &config_canonical);
    let mut restart_required = census
        .sections
        .iter()
        .any(|section| RESTART_SECTIONS.contains(&section.as_str()));
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
    // The selection is irrelevant to what the apply swaps (the remote
    // catalog is the same for every profile). The pending loader resolves
    // the state file the way the next boot would, which admits a stale or
    // absent one, and the selection it resolved is then dropped: a
    // persisted name can differ from the running profile (a switch that
    // persisted a new name and is waiting on a restart) and must not be
    // published as the live document's selection.
    let config = load_pending_config(config_path, &ProfileSelection::default())
        .and_then(|config| config.select_profile(None))
        .map_err(config_write_error)?;
    Ok(ApplyPlan::Reload(ApplySnapshot {
        config: Box::new(config),
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

/// The `ApplyConfig` command body: under the `"Applying configuration"`
/// text, swaps the remote routing table live and promotes the captured
/// shadows. The activity drops with the return on every path.
///
/// Any failure under a fired token reports as the cancellation it is, so
/// the route's reply can promise the shadows are still staged.
pub(crate) async fn apply_config(
    state: &AppState,
    snapshot: ApplySnapshot,
    token: CancellationToken,
    activity: Activity,
) -> Outcome {
    let ApplySnapshot { config, files, .. } = snapshot;
    activity.set_text("Applying configuration");
    match apply_snapshot(state, *config, files, &token).await {
        Ok(summary) => Ok(summary),
        Err(_) if token.is_cancelled() => Err(apply_cancelled()),
        Err(error) => Err(error),
    }
}

fn apply_cancelled() -> GatewayError {
    GatewayError::CommandCancelled(APPLY_CONFIG_LABEL.to_owned())
}

/// Builds the new routing table, then commits under the apply lock: the
/// captures land in their real files first (a failed promotion changes
/// nothing live), and one live write swaps the routing, config, and
/// web-search state. The running local children are never touched; their
/// routing entries carry over under the new remote catalog.
async fn apply_snapshot(
    state: &AppState,
    config: Config,
    files: Vec<ShadowCapture>,
    token: &CancellationToken,
) -> Outcome {
    if token.is_cancelled() {
        return Err(apply_cancelled());
    }
    let remote = Routing::from_config(&config)
        .map_err(|error| GatewayError::switch_failed("build-routing", error))?;
    // Only queue commands change the local runtime, and this is one, so the
    // set read here is the set the swap below publishes.
    #[cfg(feature = "local")]
    let routing = {
        let live = state.live.read().await;
        remote
            .merge(live.local.models().iter().cloned())
            .map_err(|error| GatewayError::switch_failed("merge-routing", error))?
    };
    #[cfg(not(feature = "local"))]
    let routing = remote;
    #[cfg(feature = "web-search")]
    let web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    #[cfg(test)]
    state.park_at(crate::park::Phase::ApplyCommit).await;
    // The commit holds the apply lock so no save, revert, or pending read
    // interleaves with the promotion and the live swap. A revert fires the
    // token before taking this lock, so the re-check under it is what keeps
    // a cancelled apply from writing over files the user just reverted.
    let _publication = tokio::select! {
        biased;
        () = token.cancelled() => return Err(apply_cancelled()),
        guard = state.apply.lock() => guard,
    };
    if token.is_cancelled() {
        return Err(apply_cancelled());
    }
    let applied = blocking(move || promote_captures(&files)).await??;
    let mut live = state.live.write().await;
    live.routing = Arc::new(routing);
    live.config = Arc::new(config);
    #[cfg(feature = "web-search")]
    {
        live.web_search = web_search;
    }
    Ok(format!("applied {}", applied.join(", ")))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use gateway_config::{
        Config, ProfileName, ProfileSelection, profile_state_path, shadow_path, write_shadow,
    };
    use tokio_util::sync::CancellationToken;

    use super::{ApplyPlan, capture_apply};
    use crate::AppState;
    use crate::commands::Command;
    use crate::error::GatewayError;
    use crate::park::{Phase, PhasePark};
    use crate::test_support::{AdminPaths, app_state, serve_state, wait_until};

    const CONFIG: &str = r#"
config-version = 0

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
models = []

[[profile]]
name = "beta"
models = []
"#;

    /// A third remote model appended to `CONFIG`: the shape of the one
    /// change an apply reloads live.
    const GAMMA_MODEL: &str = "\n[[model]]\nname = \"gamma-model\"\ndescription = \"gamma\"\n\
                               context = 1024\nupstream = \"gamma\"\nendpoints = [\"fake\"]\n";

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
    /// for tests that read the queue or the live table.
    async fn serve_fixture(config: Config, paths: AdminPaths) -> (SocketAddr, AppState) {
        let state = app_state(config, Some(paths));
        let _worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let addr = serve_state(state.clone()).await;
        (addr, state)
    }

    /// [`serve_fixture`] with the apply command parked at its commit, so a
    /// test can act between the capture and the promotion.
    async fn serve_parked_fixture(
        config: Config,
        paths: AdminPaths,
    ) -> (SocketAddr, AppState, Arc<PhasePark>) {
        let mut state = app_state(config, Some(paths));
        let park = Arc::new(PhasePark::at(Phase::ApplyCommit));
        state.park = Some(Arc::clone(&park));
        let _worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let addr = serve_state(state.clone()).await;
        (addr, state, park)
    }

    /// Stages `CONFIG` plus `GAMMA_MODEL` as the pending config.
    fn stage_gamma(config_path: &std::path::Path) {
        write_shadow(config_path, &format!("{CONFIG}{GAMMA_MODEL}")).expect("stage config shadow");
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

    /// Saves the live config with `edit` applied through the real save
    /// route, so the shadow is exactly what the UI would write: the running
    /// `active_profile` that `GET /admin/config` reports is not a
    /// configuration key and never goes back in a save.
    async fn save_edited(
        addr: SocketAddr,
        edit: impl FnOnce(&mut serde_json::Value),
    ) -> reqwest::Response {
        let mut body = get_json(addr, "admin/config").await;
        body.as_object_mut()
            .expect("the config is an object")
            .remove("active_profile");
        edit(&mut body);
        reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("save sends")
    }

    /// The live profile name, as `GET /admin/status` would report it.
    async fn live_profile(state: &AppState) -> Option<String> {
        state.live.read().await.profile_name.clone()
    }

    /// Whether the live routing table resolves `name`.
    async fn routes(state: &AppState, name: &str) -> bool {
        state.live.read().await.routing.model(name).is_ok()
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

    /// A child-free local runtime holding one model named `name`, standing
    /// in for a running `llama-server` the apply must keep routing.
    #[cfg(feature = "test-fixtures")]
    fn running_local(name: &str) -> crate::local::LocalRuntime {
        let config = Config::from_toml_str(&format!(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"{name}\"\ndescription = \"running child\"\n\
             context = 4096\nupstream = \"{name}\"\nendpoints = [\"local\"]\n"
        ))
        .expect("local fixture config parses");
        let routing =
            crate::routing::Routing::from_config(&config).expect("local fixture routing builds");
        crate::local::LocalRuntime::from_test_models(routing.models().to_vec())
    }

    /// The reload an apply performs: a new `[[model]]` enters the live
    /// routing table, the running local child keeps its entry, the shadow
    /// promotes, and the reply says so without a restart.
    #[cfg(feature = "test-fixtures")]
    #[tokio::test]
    async fn apply_with_a_new_model_swaps_the_routing_live_and_promotes_the_shadow() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, state) = serve_fixture(config, paths).await;
        {
            let mut live = state.live.write().await;
            live.local = running_local("alpha-local");
            let routing = Arc::clone(&live.routing);
            live.routing = Arc::new(
                routing
                    .as_ref()
                    .clone()
                    .merge(live.local.models().iter().cloned())
                    .expect("the running child routes"),
            );
        }
        stage_gamma(&config_path);

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["restart_required"], false);
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
        assert!(routes(&state, "gamma-model").await, "the new model routes");
        assert!(
            routes(&state, "alpha-local").await,
            "the running local child keeps its routing entry"
        );
        assert_eq!(
            state.live.read().await.local.child_count(),
            1,
            "the local runtime is untouched"
        );
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
        assert!(!shadow_path(&config_path).exists(), "the shadow promoted");
        assert!(
            std::fs::read_to_string(&config_path)
                .expect("read applied config")
                .contains("gamma-model"),
            "the real file carries the applied change"
        );
        let served = get_json(addr, "admin/config").await;
        assert_eq!(served["model"][2]["name"], "gamma-model");
        assert!(
            !state.hub.current().busy,
            "the settled apply released its activity: {:?}",
            state.hub.current()
        );
    }

    /// A persisted selection that differs from the running profile (a
    /// switch that persisted a new name and awaits a restart) never reaches
    /// the live document: the applied config carries no selection, the
    /// running profile is unchanged, and `GET /admin/config` does not report
    /// the persisted name as the running one.
    #[tokio::test]
    async fn apply_publishes_the_document_without_the_persisted_selection() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, state) = serve_fixture(config, paths).await;
        std::fs::write(
            profile_state_path(&config_path),
            "active_profile = \"beta\"\n",
        )
        .expect("persist a selection awaiting restart");
        stage_gamma(&config_path);

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(routes(&state, "gamma-model").await);
        assert!(
            state.live.read().await.config.active_profile().is_none(),
            "the live document carries no selection"
        );
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
        let served = get_json(addr, "admin/config").await;
        assert!(
            served.get("active_profile").is_none(),
            "the persisted name is not reported as running: {served}"
        );
    }

    /// Each boot-read section flags a restart; the live-reloadable ones do
    /// not. Every case stages a valid config differing from the real one in
    /// exactly that section.
    #[test]
    fn capture_apply_flags_restart_for_boot_read_sections_only() {
        let cases: [(&str, &str, bool); 7] = [
            (
                "profile",
                "\n[[profile]]\nname = \"gamma\"\nmodels = []\n",
                true,
            ),
            (
                "local_model",
                "\n[[local_model]]\nname = \"gamma\"\ndescription = \"g\"\n\
                 source = \"/models/gamma.gguf\"\ncontext = 4096\n",
                true,
            ),
            (
                "stt_model",
                "\n[[stt_model]]\nname = \"speech\"\nrole = \"interim\"\n\
                 source = \"/speech.bin\"\nvram_gb = 1.0\n",
                true,
            ),
            (
                "stt",
                "\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\n",
                true,
            ),
            ("model", GAMMA_MODEL, false),
            (
                "endpoint",
                "\n[[endpoint]]\nid = \"other\"\nprotocol = \"openai\"\n\
                 base_url = \"http://127.0.0.1:10\"\napi_key = \"\"\n",
                false,
            ),
            (
                "tools",
                "\n[tools.web_search]\nprovider = \"brave\"\napi_key = \"k\"\n",
                false,
            ),
        ];
        for (section, addition, expected) in cases {
            let temp = tempfile::TempDir::new().expect("temp dir");
            let config_path = temp.path().join("gateway.toml");
            std::fs::write(&config_path, CONFIG).expect("write config");
            write_shadow(&config_path, &format!("{CONFIG}{addition}")).expect("stage shadow");

            let plan = capture_apply(&config_path).expect("the pending config captures");

            let ApplyPlan::Reload(snapshot) = plan else {
                panic!("a config shadow always reloads: {section}");
            };
            assert_eq!(
                snapshot.restart_required, expected,
                "restart_required for a {section} change"
            );
            assert_eq!(snapshot.applied_names(), ["gateway.toml"]);
        }
    }

    #[tokio::test]
    async fn invalid_pending_config_is_never_promoted() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        write_shadow(&config_path, "not valid TOML [[[").expect("stage tampered shadow");
        let (addr, state) = serve_fixture(config, paths).await;

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config
        );
        assert!(
            shadow_path(&config_path).is_file(),
            "the rejected shadow remains available for correction or revert"
        );
        assert!(
            !state.hub.current().busy,
            "the parse failure replies before any command exists"
        );
        assert!(state.commands.active_command().is_none());
        assert!(state.commands.pending_commands().is_empty());
    }

    #[tokio::test]
    async fn env_only_apply_requires_restart_without_a_command() {
        let (_temp, config, paths) = fixture();
        let env_path = paths.config_path.with_extension("env");
        write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env shadow");
        let (addr, state) = serve_fixture(config, paths).await;

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
        assert!(
            !state.hub.current().busy && state.commands.active_command().is_none(),
            "the no-reload path promotes inline without a command"
        );
    }

    #[tokio::test]
    async fn server_key_change_waits_for_restart() {
        let (_temp, config, paths) = fixture();
        let (addr, _state) = serve_fixture(config, paths).await;
        let http = reqwest::Client::new();
        let save = save_edited(addr, |body| {
            body["server"]["api_key"] = serde_json::json!("next-token");
        })
        .await;
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

    /// The speech pipeline is configured once at boot: an `[stt]` change
    /// promotes and reloads the document but reports a restart.
    #[tokio::test]
    async fn stt_pipeline_change_promotes_and_requires_restart() {
        let (_temp, config, paths) = fixture();
        write_shadow(
            &paths.config_path,
            &format!(
                "{CONFIG}\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\n\
                 vocabulary = [\"WG21\"]\n"
            ),
        )
        .expect("stage STT-only shadow");
        let (addr, _state) = serve_fixture(config, paths).await;
        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty["changed_sections"], serde_json::json!(["stt"]));

        let response = post(addr, "admin/config-apply").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["restart_required"], true);
        let applied = get_json(addr, "admin/config").await;
        assert_eq!(applied["stt"]["window_seconds"], 8);
        assert_eq!(applied["stt"]["interval_ms"], 250);
        assert_eq!(applied["stt"]["vocabulary"], serde_json::json!(["WG21"]));
    }

    #[tokio::test]
    async fn revert_removes_all_shadows_without_touching_real_files() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let env_path = config_path.with_extension("env");
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        stage_gamma(&config_path);
        write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env");
        let (addr, _state) = serve_fixture(config, paths).await;

        let response = post(addr, "admin/config-revert").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("revert body");
        assert_eq!(
            reply["reverted"],
            serde_json::json!(["gateway.env.next", "gateway.toml.next"])
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config
        );
        assert!(!env_path.exists());
    }

    /// Two applies in flight at once share one command: the second attaches
    /// to the first through the debounce, both replies carry the same
    /// `applied` list, and the reload runs exactly once.
    #[tokio::test]
    async fn concurrent_applies_promote_the_pending_config_once() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, state, park) = serve_parked_fixture(config, paths).await;
        stage_gamma(&config_path);

        let first = tokio::spawn(post(addr, "admin/config-apply"));
        park.entered().await;
        assert_eq!(
            state.hub.current().text,
            "Applying configuration",
            "the parked apply holds the command's activity with its stage text"
        );
        let second = tokio::spawn(post(addr, "admin/config-apply"));
        wait_until("the second apply to attach to the first", || {
            state.commands.active_waiters() == 2
        })
        .await;
        assert!(
            state.commands.pending_commands().is_empty(),
            "the second apply attached to the active one instead of queueing"
        );
        park.release();

        let first = first.await.expect("first apply task");
        let second = second.await.expect("second apply task");
        assert_eq!(first.status(), reqwest::StatusCode::OK);
        assert_eq!(second.status(), reqwest::StatusCode::OK);
        let first: serde_json::Value = first.json().await.expect("first body");
        let second: serde_json::Value = second.json().await.expect("second body");
        let expected = serde_json::json!(["gateway.toml"]);
        assert_eq!(first["applied"], expected);
        assert_eq!(
            second["applied"], expected,
            "both replies report the shared outcome"
        );
        assert_eq!(first["reloaded"], true);
        assert_eq!(second["reloaded"], true);
        assert!(
            !state.hub.current().busy,
            "the one command settled and released its activity"
        );
        assert!(!shadow_path(&config_path).exists());
        assert!(routes(&state, "gamma-model").await);
    }

    /// An apply enqueued after the boot `LoadProfile` never displaces it:
    /// the boot load settles on its own terms over the production worker,
    /// then the apply runs over the table it published and completes. The
    /// queue's FIFO rule under an active boot load is pinned in
    /// `commands.rs`.
    #[tokio::test]
    async fn an_apply_after_the_boot_load_reloads_over_the_published_table() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, state) = serve_fixture(config, paths).await;
        stage_gamma(&config_path);

        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));
        wait_until("the boot load to settle", || {
            state.commands.active_command().is_none()
        })
        .await;
        let outcome = tokio::time::timeout(Duration::from_secs(10), boot.outcome)
            .await
            .expect("the boot load settles")
            .expect("the boot load settles with an outcome");
        assert!(outcome.is_ok(), "a remote-only profile loads: {outcome:?}");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
        assert!(!shadow_path(&config_path).exists());
        assert!(routes(&state, "gamma-model").await);
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
    }

    /// A cancelled apply promotes nothing: the shadow stays on disk with its
    /// contents, the dirty report is unchanged, the reply is the
    /// cancellation envelope, and a retry applies the same change.
    #[tokio::test]
    async fn a_cancelled_apply_leaves_every_shadow_staged_and_a_retry_succeeds() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        let (addr, state, park) = serve_parked_fixture(config, paths).await;
        stage_gamma(&config_path);
        let staged = std::fs::read_to_string(shadow_path(&config_path)).expect("staged shadow");
        let dirty_before = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty_before["dirty"], true);

        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        park.entered().await;
        assert!(state.commands.cancel_active());
        park.release();

        assert_apply_cancelled(apply.await.expect("apply task")).await;
        assert_eq!(
            std::fs::read_to_string(shadow_path(&config_path)).expect("config shadow"),
            staged,
            "the config shadow is still staged"
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config,
            "nothing was promoted"
        );
        assert_eq!(
            get_json(addr, "admin/config-dirty").await,
            dirty_before,
            "the dirty report is unchanged"
        );
        assert!(!routes(&state, "gamma-model").await, "nothing went live");

        // The retry parks at the same phase; a stored release lets it through.
        park.release();
        let retry = post(addr, "admin/config-apply").await;
        assert_eq!(retry.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = retry.json().await.expect("retry body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
        assert!(!shadow_path(&config_path).exists());
        assert!(routes(&state, "gamma-model").await);
    }

    /// A save that lands mid-apply neither blocks nor is lost: the snapshot's
    /// contents land in the real file, and the newer shadow stays pending as
    /// the next change.
    #[tokio::test]
    async fn a_save_landing_mid_apply_stays_pending_while_the_snapshot_lands() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let (addr, state, park) = serve_parked_fixture(config, paths).await;
        stage_gamma(&config_path);

        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        park.entered().await;
        let save = tokio::time::timeout(
            Duration::from_secs(10),
            save_edited(addr, |body| {
                body["model"][0]["description"] = serde_json::json!("edited mid-apply");
            }),
        )
        .await
        .expect("the save completes while the apply is active");
        assert_eq!(save.status(), reqwest::StatusCode::OK);
        park.release();

        let response = apply.await.expect("apply task");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("apply body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
        let real = std::fs::read_to_string(&config_path).expect("read config");
        assert!(
            real.contains("gamma-model") && !real.contains("edited mid-apply"),
            "the snapshot's contents landed in the real file"
        );
        let pending = std::fs::read_to_string(shadow_path(&config_path)).expect("config shadow");
        assert!(
            pending.contains("edited mid-apply"),
            "the newer save stays pending instead of being deleted"
        );
        assert!(routes(&state, "gamma-model").await);
        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty["pending_files"], serde_json::json!(["gateway.toml"]));
    }

    /// A revert during an active apply wins: the apply settles as cancelled,
    /// its commit writes nothing, and the shadow is gone.
    #[tokio::test]
    async fn a_revert_during_an_active_apply_cancels_it_and_the_commit_writes_nothing() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let original_config = std::fs::read_to_string(&config_path).expect("read config");
        let (addr, state, park) = serve_parked_fixture(config, paths).await;
        stage_gamma(&config_path);

        let apply = tokio::spawn(post(addr, "admin/config-apply"));
        park.entered().await;

        let revert = post(addr, "admin/config-revert").await;
        assert_eq!(revert.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = revert.json().await.expect("revert body");
        assert_eq!(reply["reverted"], serde_json::json!(["gateway.toml.next"]));
        park.release();

        assert_apply_cancelled(apply.await.expect("apply task")).await;
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("re-read config"),
            original_config,
            "the cancelled apply's commit wrote nothing"
        );
        assert!(!shadow_path(&config_path).exists());
        assert!(!routes(&state, "gamma-model").await);
        assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
        wait_until("the queue to go idle", || {
            state.commands.active_command().is_none()
        })
        .await;
    }
}
