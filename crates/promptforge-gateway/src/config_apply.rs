//! Apply and revert routes for pending config shadows:
//! `POST /admin/config-apply` and `POST /admin/config-revert`.
//!
//! Apply promotes every shadow to its real file by atomic rename, then
//! reloads the active profile through the same machinery as
//! `POST /admin/switch-profile` when any profile-scoped or `.env` shadow
//! was promoted; a promoted boot shadow reports `restart_required` instead,
//! because the gateway cannot hot-reload its boot-owned sections. Revert
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
use promptforge_gateway_config::{ProfileName, promote_shadow, shadow_path};

use crate::config_pending::{canonical_form, config_root, relative_name, shadow_census};
use crate::config_write::{active_profile_path, config_write_error, error_chain};
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `POST /admin/config-apply` route: bearer-authed, promotes every
/// shadow to its real file, then reloads the active profile when needed.
///
/// The reply is plain JSON - `{"applied": [...], "reloaded": bool,
/// "restart_required": bool}` - not SSE: the reload runs through the same
/// path as `POST /admin/switch-profile`, so its staged progress streams to
/// `GET /admin/progress` subscribers, and the response carries the
/// outcome. `applied` names the promoted real files relative to the config
/// root, sorted. `reloaded` is true when a profile-scoped or profile
/// `.env` shadow was promoted and the reload succeeded. `restart_required`
/// is true when the boot config's or the boot `.env`'s shadow was
/// promoted, because boot-owned state loads only on the next restart.
/// With no shadows on disk the reply is the clean no-op
/// `{"applied": [], "reloaded": false, "restart_required": false}`.
///
/// Promotion happens before the reload, so a reload failure leaves the
/// gateway running the previous configuration while the real files already
/// carry the new one: the error reply
/// ([`GatewayError::ApplyReloadFailed`], 500) notes that the new config
/// loads on the next restart.
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
    let leaf = active_profile_path(&state).await?;
    let boot = state.boot.config_path.clone();
    let promotion = tokio::task::spawn_blocking(move || promote_all(&leaf, boot.as_deref()))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
    let mut reloaded = false;
    if promotion.reload {
        reload_active_profile(&state)
            .await
            .map_err(|error| GatewayError::ApplyReloadFailed(error_chain(&error)))?;
        reloaded = true;
    }
    Ok(Json(serde_json::json!({
        "applied": promotion.applied,
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
    let leaf = active_profile_path(&state).await?;
    let boot = state.boot.config_path.clone();
    let reverted = tokio::task::spawn_blocking(move || delete_all_shadows(&leaf, boot.as_deref()))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))??;
    Ok(Json(serde_json::json!({ "reverted": reverted })))
}

/// What one apply promoted: the rendered file names and the classification
/// deciding the follow-up (reload, restart banner, or neither).
struct Promotion {
    /// Promoted real files relative to the config root, sorted.
    applied: Vec<String>,
    /// Whether any profile-scoped or profile `.env` shadow was promoted,
    /// requiring a profile reload.
    reload: bool,
    /// Whether the boot config's or the boot `.env`'s shadow was promoted;
    /// boot-owned state applies only on the next restart.
    restart_required: bool,
}

/// Promotes every shadow the census finds, classifying each promoted file
/// as boot-scoped (restart required) or profile-scoped/env (reload).
fn promote_all(leaf: &Path, boot: Option<&Path>) -> Result<Promotion, GatewayError> {
    let census = shadow_census(leaf, boot)?;
    let root = config_root(leaf, boot);
    let boot_canonical = boot.map(canonical_form);
    // The boot `.env` is restart-scoped like the boot config itself: the
    // runner loads it once at startup, and a profile reload re-reads only
    // the profile's own env file.
    let boot_env_canonical = boot.map(|boot| canonical_form(&boot.with_extension("env")));
    let mut promotion = Promotion {
        applied: Vec::new(),
        reload: false,
        restart_required: false,
    };
    for file in &census.files {
        promote_shadow(file).map_err(config_write_error)?;
        let boot_scoped = boot_canonical.as_deref() == Some(file.as_path())
            || boot_env_canonical.as_deref() == Some(file.as_path());
        if boot_scoped {
            promotion.restart_required = true;
        } else {
            promotion.reload = true;
        }
        promotion.applied.push(relative_name(file, root));
    }
    promotion.applied.sort_unstable();
    Ok(promotion)
}

/// Deletes every shadow the census finds, returning the deleted shadow
/// files relative to the config root, sorted.
fn delete_all_shadows(leaf: &Path, boot: Option<&Path>) -> Result<Vec<String>, GatewayError> {
    let census = shadow_census(leaf, boot)?;
    let root = config_root(leaf, boot);
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

/// Reloads the active profile through the switch machinery: `run_switch`
/// on a fresh operation tree, awaited to completion so the apply response
/// can report the outcome. The tree attaches to the process progress hub,
/// so the reload's stages stream to `GET /admin/progress` subscribers
/// exactly as a `POST /admin/switch-profile` would.
async fn reload_active_profile(state: &AppState) -> Result<(), GatewayError> {
    let dir = crate::profiles_dir(state)?.to_path_buf();
    let name = state
        .live
        .read()
        .await
        .profile_name
        .clone()
        .ok_or(GatewayError::ProfilesUnavailable)?;
    let name = ProfileName::parse(&name)
        .map_err(|error| GatewayError::switch_failed("parse-name", error))?;
    let tree = state.hub.operation();
    crate::run_switch(state.clone(), dir, name, tree).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use promptforge_gateway_config::{Config, ProfileName, shadow_path, write_shadow};

    use crate::test_support::{AdminPaths, serve_with_paths};

    /// The boot catalog: server key, one endpoint with a literal secret,
    /// one model.
    const BOOT: &str = r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "boot-endpoint-secret"

[[model]]
name = "m"
description = "from the boot file"
context = 1024
upstream = "u"
endpoints = ["e"]
"#;

    /// An included parent with its own endpoint secret.
    const COMMON: &str = r#"
[[endpoint]]
id = "e2"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "common-endpoint-secret"
"#;

    const MAIN: &str = "include = [\"../gateway.toml\", \"common.toml\"]\n";

    /// A tempdir holding `gateway.toml`, `profiles/common.toml`, and a
    /// `main` profile including both, plus the loaded config and paths.
    fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(temp.path().join("gateway.toml"), BOOT).expect("write boot");
        let profiles = temp.path().join("profiles");
        std::fs::create_dir(&profiles).expect("mkdir profiles");
        std::fs::write(profiles.join("common.toml"), COMMON).expect("write common");
        std::fs::write(profiles.join("main.toml"), MAIN).expect("write main");
        let config = Config::load_profile(&profiles, &ProfileName::parse("main").expect("name"))
            .expect("the fixture profile loads");
        let paths = AdminPaths {
            profiles_dir: profiles,
            active: "main".to_owned(),
            boot_config: temp.path().join("gateway.toml"),
        };
        (temp, config, paths)
    }

    /// Every real config file in the fixture, with its current bytes.
    fn real_files(root: &Path) -> Vec<(PathBuf, String)> {
        [
            root.join("gateway.toml"),
            root.join("profiles").join("common.toml"),
            root.join("profiles").join("main.toml"),
        ]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read_to_string(&path).expect("read fixture file");
            (path, bytes)
        })
        .collect()
    }

    /// Shadow files (`*.next`) present anywhere in the fixture tree.
    fn shadow_names(root: &Path) -> Vec<String> {
        let mut names = Vec::new();
        for dir in [root.to_path_buf(), root.join("profiles")] {
            for entry in std::fs::read_dir(dir).expect("read dir") {
                let name = entry.expect("dir entry").file_name();
                if name.to_string_lossy().ends_with(".next") {
                    names.push(name.to_string_lossy().into_owned());
                }
            }
        }
        names.sort_unstable();
        names
    }

    async fn get_json(addr: std::net::SocketAddr, route: &str) -> serde_json::Value {
        reqwest::Client::new()
            .get(format!("http://{addr}/{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends")
            .json()
            .await
            .expect("a JSON body")
    }

    async fn post(addr: std::net::SocketAddr, route: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends")
    }

    #[tokio::test]
    async fn apply_with_a_profile_shadow_promotes_it_and_reloads() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        // Stage through the step 7 save path: the config JSON round-trips
        // with one edit.
        let mut body = get_json(addr, "admin/config").await;
        body["model"][0]["description"] = serde_json::json!("edited in the UI");
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(reply["applied"], serde_json::json!(["profiles/main.toml"]));
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["restart_required"], false);

        assert!(
            !shadow_path(&leaf).exists(),
            "the shadow disappears in the promotion"
        );
        let promoted: toml::Value =
            toml::from_str(&std::fs::read_to_string(&leaf).expect("read leaf"))
                .expect("the promoted leaf parses");
        assert_eq!(
            promoted["model"][0]["description"].as_str(),
            Some("edited in the UI"),
            "the real file carries the shadow's content"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["model"][0]["description"],
            "edited in the UI",
            "the reload ran: the running config carries the applied edit"
        );
        drop(temp);
    }

    #[tokio::test]
    async fn apply_with_a_boot_shadow_requires_restart_and_skips_the_reload() {
        let (temp, config, paths) = fixture();
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(
            &boot,
            &BOOT.replace("from the boot file", "edited boot entry"),
        )
        .expect("stage the boot shadow");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
        assert_eq!(
            reply["reloaded"], false,
            "a boot-only promotion attempts no reload"
        );
        assert_eq!(reply["restart_required"], true);

        assert!(!shadow_path(&boot).exists(), "the boot shadow is gone");
        assert!(
            std::fs::read_to_string(&boot)
                .expect("read boot")
                .contains("edited boot entry"),
            "the real boot file carries the shadow's content"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["model"][0]["description"],
            "from the boot file",
            "no reload: the running config still serves the old value"
        );
    }

    #[tokio::test]
    async fn apply_with_profile_and_boot_shadows_promotes_both_and_reloads() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(
            &boot,
            &BOOT.replace("from the boot file", "edited boot entry"),
        )
        .expect("stage the boot shadow");
        write_shadow(
            &leaf,
            &format!(
                "{MAIN}\n[[model]]\nname = \"m2\"\ndescription = \"added pending\"\n\
                 context = 512\nupstream = \"u2\"\nendpoints = [\"e\"]\n"
            ),
        )
        .expect("stage the leaf shadow");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            reply["applied"],
            serde_json::json!(["gateway.toml", "profiles/main.toml"])
        );
        assert_eq!(reply["reloaded"], true);
        assert_eq!(reply["restart_required"], true);

        assert_eq!(
            shadow_names(temp.path()),
            Vec::<String>::new(),
            "no shadow survives the apply"
        );
        let running = get_json(addr, "admin/config").await;
        assert_eq!(
            running["model"][1]["description"], "added pending",
            "the reload picked up the promoted leaf: {running}"
        );
        assert_eq!(
            running["model"][0]["description"], "edited boot entry",
            "the profile chain includes the promoted boot file"
        );
    }

    #[tokio::test]
    async fn apply_with_no_shadows_is_a_clean_no_op() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            reply,
            serde_json::json!({
                "applied": [],
                "reloaded": false,
                "restart_required": false,
            })
        );
    }

    #[tokio::test]
    async fn apply_classifies_env_shadows_by_scope() {
        // The profile `.env` reloads with the profile; the boot `.env`
        // loads only at startup, so its promotion is restart-scoped.
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(&leaf.with_extension("env"), "HF_TOKEN=pending\n")
            .expect("stage the profile env shadow");
        write_shadow(&boot.with_extension("env"), "BOOT_KEY=pending\n")
            .expect("stage the boot env shadow");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            reply["applied"],
            serde_json::json!(["gateway.env", "profiles/main.env"])
        );
        assert_eq!(
            reply["reloaded"], true,
            "the profile env shadow triggers the reload"
        );
        assert_eq!(
            reply["restart_required"], true,
            "the boot env loads only at startup, so its promotion requires a restart"
        );
        assert_eq!(
            shadow_names(temp.path()),
            Vec::<String>::new(),
            "both env shadows promoted"
        );
    }

    #[tokio::test]
    async fn apply_with_only_a_boot_env_shadow_skips_the_reload() {
        let (temp, config, paths) = fixture();
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(&boot.with_extension("env"), "BOOT_KEY=pending\n")
            .expect("stage the boot env shadow");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(reply["applied"], serde_json::json!(["gateway.env"]));
        assert_eq!(
            reply["reloaded"], false,
            "nothing profile-scoped promoted, so no reload runs"
        );
        assert_eq!(reply["restart_required"], true);
        drop(temp);
    }

    #[tokio::test]
    async fn a_mid_apply_promotion_failure_is_recoverable_by_a_second_apply() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let env = leaf.with_extension("env");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(
            &leaf,
            &format!(
                "{MAIN}\n[[model]]\nname = \"m2\"\ndescription = \"added pending\"\n\
                 context = 512\nupstream = \"u2\"\nendpoints = [\"e\"]\n"
            ),
        )
        .expect("stage the leaf shadow");
        write_shadow(&env, "HF_TOKEN=pending\n").expect("stage the env shadow");
        // A directory squatting on the env target defeats promotion's
        // rename and its remove-then-rename fallback alike, so the apply
        // fails after the leaf already promoted.
        std::fs::create_dir(&env).expect("squat a directory on the env target");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body: serde_json::Value = response.json().await.expect("a JSON envelope");
        assert_eq!(body["error"]["code"], "config_write_error");
        assert!(
            !shadow_path(&leaf).exists(),
            "promotion is per file: the leaf promoted before the env failed"
        );
        assert!(
            shadow_path(&env).exists(),
            "the failed file's shadow survives for the retry"
        );
        assert_eq!(
            get_json(addr, "admin/config-dirty").await["pending_files"],
            serde_json::json!(["profiles/main.env"]),
            "the dirty report names exactly the unpromoted remainder"
        );

        std::fs::remove_dir(&env).expect("clear the blocker");
        let response = post(addr, "admin/config-apply").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            reply["applied"],
            serde_json::json!(["profiles/main.env"]),
            "the second apply promotes the remainder"
        );
        assert_eq!(reply["reloaded"], true);
        assert_eq!(
            get_json(addr, "admin/config-dirty").await["dirty"],
            false,
            "nothing stays pending after the recovery apply"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["model"][1]["description"],
            "added pending",
            "the recovery reload picked up the first apply's promoted leaf"
        );
        drop(temp);
    }

    #[tokio::test]
    async fn concurrent_applies_serialize_on_the_apply_mutex() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(
            &leaf,
            &format!(
                "{MAIN}\n[[model]]\nname = \"m2\"\ndescription = \"added pending\"\n\
                 context = 512\nupstream = \"u2\"\nendpoints = [\"e\"]\n"
            ),
        )
        .expect("stage the leaf shadow");

        // Without the mutex the loser would race the winner's promotion
        // and 500 on "no shadow to promote"; serialized, the loser
        // observes the applied state as a clean no-op.
        let (first, second) = tokio::join!(
            post(addr, "admin/config-apply"),
            post(addr, "admin/config-apply")
        );
        assert_eq!(first.status(), reqwest::StatusCode::OK);
        assert_eq!(second.status(), reqwest::StatusCode::OK);
        let first: serde_json::Value = first.json().await.expect("a JSON body");
        let second: serde_json::Value = second.json().await.expect("a JSON body");
        let mut applied = [first["applied"].clone(), second["applied"].clone()];
        applied.sort_unstable_by_key(|value| value.as_array().map_or(0, Vec::len));
        assert_eq!(
            applied,
            [
                serde_json::json!([]),
                serde_json::json!(["profiles/main.toml"]),
            ],
            "exactly one request promotes; the other sees no shadows"
        );
        assert!(
            !shadow_path(&leaf).exists(),
            "the shadow is promoted exactly once"
        );
        drop(temp);
    }

    #[tokio::test]
    async fn revert_deletes_every_shadow_and_rewrites_nothing() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let common = paths.profiles_dir.join("common.toml");
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;
        let before = real_files(temp.path());

        write_shadow(&leaf, MAIN).expect("stage the leaf shadow");
        write_shadow(&common, &COMMON.replace(":9", ":10")).expect("stage the include shadow");
        write_shadow(
            &boot,
            &BOOT.replace("from the boot file", "edited boot entry"),
        )
        .expect("stage the boot shadow");
        write_shadow(&leaf.with_extension("env"), "HF_TOKEN=pending\n")
            .expect("stage the profile env shadow");
        write_shadow(&boot.with_extension("env"), "BOOT_KEY=pending\n")
            .expect("stage the boot env shadow");

        let response = post(addr, "admin/config-revert").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            reply["reverted"],
            serde_json::json!([
                "gateway.env.next",
                "gateway.toml.next",
                "profiles/common.toml.next",
                "profiles/main.env.next",
                "profiles/main.toml.next",
            ])
        );

        assert_eq!(
            shadow_names(temp.path()),
            Vec::<String>::new(),
            "every shadow is deleted"
        );
        for (path, bytes) in before {
            assert_eq!(
                std::fs::read_to_string(&path).expect("re-read fixture file"),
                bytes,
                "{} is byte-identical after the revert",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn a_reload_failure_after_promotion_notes_next_restart_semantics() {
        let (_temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        // Written directly, bypassing save-time validation: the shadow
        // parses (so enumeration and promotion succeed) but its merged
        // config fails validation, so the reload after promotion fails.
        write_shadow(
            &leaf,
            &format!(
                "{MAIN}\n[[model]]\nname = \"bad\"\ndescription = \"references a ghost\"\n\
                 context = 1\nupstream = \"u\"\nendpoints = [\"ghost\"]\n"
            ),
        )
        .expect("stage the invalid leaf shadow");

        let response = post(addr, "admin/config-apply").await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body: serde_json::Value = response.json().await.expect("a JSON envelope");
        assert_eq!(body["error"]["code"], "apply_reload_failed");
        let message = body["error"]["message"].as_str().expect("a message");
        assert!(
            message.contains("next restart"),
            "the error notes the next-restart semantics: {message}"
        );
        assert!(
            message.contains("ghost"),
            "the error carries the reload failure's cause: {message}"
        );

        assert!(
            !shadow_path(&leaf).exists(),
            "promotion happened before the reload failed"
        );
        assert!(
            std::fs::read_to_string(&leaf)
                .expect("read leaf")
                .contains("ghost"),
            "the real file carries the promoted content"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["model"][0]["description"],
            "from the boot file",
            "the gateway keeps running the old config after a failed reload"
        );
    }

    #[tokio::test]
    async fn apply_and_revert_require_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        for route in ["admin/config-apply", "admin/config-revert"] {
            let response = http
                .post(format!("http://{addr}/{route}"))
                .send()
                .await
                .expect("the request sends");
            assert_eq!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "{route} must refuse a request without the bearer key"
            );
        }
    }
}
