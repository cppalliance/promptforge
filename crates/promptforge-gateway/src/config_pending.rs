//! Pending-state read routes: `GET /admin/config-pending` and
//! `GET /admin/config-dirty`.
//!
//! The write routes (`config_write.rs`) stage edits as `.next` shadows
//! beside the real files; these routes read that pending state back.
//! `config-pending` renders the merged pending configuration - the active
//! profile's include chain resolved with existing shadows preferred - in
//! the exact `GET /admin/config` shape, secrets `"***"`, provenance
//! naming the `.next` file wherever a shadow's definition won.
//! `config-dirty` is the cheap poll: whether any shadow exists, which
//! real files carry one, and which top-level sections the pending view
//! changes. The resolution machinery lives in
//! `promptforge-gateway-config`; these handlers own auth, path assembly,
//! and the wire shape.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use promptforge_gateway_config::{load_pending_profile, pending_report, shadow_path};

use crate::config_write::active_profile_path;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `GET /admin/config-pending` route: bearer-authed, renders the
/// merged pending configuration.
///
/// The reply is `{"profile": ..., "boot": ...}`. `profile` is the active
/// profile's include chain resolved with existing shadows preferred,
/// serialized exactly like `GET /admin/config`: same JSON shape, secrets
/// `"***"`, provenance naming the shadow file (`<file>.toml.next`)
/// wherever the winning definition came from a shadow, and the top-level
/// `include` array carrying the leaf's own include line verbatim and
/// ordered - the leaf shadow's when one exists. With no shadows on disk
/// `profile` equals the `GET /admin/config` payload. `boot` is `null`
/// until the boot config has a shadow, then
/// `{"shadow", "changed_sections"}` - kept apart from the profile view so
/// the UI can raise the restart-required banner for boot edits. The boot
/// object carries no `include` array by design: the UI's chain editor
/// edits only the active profile's chain, so the boot file's own includes
/// are out of scope here (their merged values still flow into `profile`
/// wherever the profile chain reaches the boot file).
pub(crate) async fn admin_config_pending(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let leaf = active_profile_path(&state).await?;
    let boot = state.boot.config_path.clone();
    let reply = tokio::task::spawn_blocking(move || {
        let profile = load_pending_profile(&leaf)
            .map_err(|error| pending_read_error(&error))?
            .to_json();
        Ok::<_, GatewayError>(serde_json::json!({
            "profile": profile,
            "boot": boot_pending(boot.as_deref())?,
        }))
    })
    .await
    .map_err(|join| GatewayError::PendingConfig(join.to_string()))??;
    Ok(Json(reply))
}

/// The `GET /admin/config-dirty` route: bearer-authed, reports the
/// pending state from shadow existence and comparison.
///
/// The reply is `{"dirty", "pending_files", "changed_sections"}`. `dirty`
/// is true when any shadow exists. `pending_files` names the real files
/// whose shadows are present - the profile chain, the boot config, and
/// the profile and boot `.env` siblings - rendered relative to the boot
/// config's directory with forward slashes, sorted. `changed_sections` is
/// the sorted union of top-level sections whose merged value differs
/// between the real and pending views of the profile and boot chains;
/// `.env` shadows count toward `dirty` and `pending_files` only, since no
/// TOML section carries them.
pub(crate) async fn admin_config_dirty(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let leaf = active_profile_path(&state).await?;
    let boot = state.boot.config_path.clone();
    let reply = tokio::task::spawn_blocking(move || dirty_reply(&leaf, boot.as_deref()))
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

/// The boot side of the pending reply: `null` until the boot config has a
/// shadow, then the shadow path and the top-level sections the pending
/// boot view changes.
fn boot_pending(boot: Option<&Path>) -> Result<serde_json::Value, GatewayError> {
    let Some(boot) = boot else {
        return Ok(serde_json::Value::Null);
    };
    let shadow = shadow_path(boot);
    if !shadow.is_file() {
        return Ok(serde_json::Value::Null);
    }
    let report = pending_report(boot).map_err(|error| pending_read_error(&error))?;
    Ok(serde_json::json!({
        "shadow": shadow.display().to_string(),
        "changed_sections": report.changed_sections,
    }))
}

/// Every shadow on disk for one gateway: the real files that carry one
/// (profile chain, boot chain, `.env` siblings) and the top-level sections
/// the shadows change. The dirty report renders it, and the apply and
/// revert routes enumerate from it.
pub(crate) struct ShadowCensus {
    /// Real files whose shadows exist, in canonical form, without
    /// duplicates.
    pub(crate) files: Vec<PathBuf>,
    /// Top-level sections whose merged value the shadows change, sorted
    /// and deduplicated.
    pub(crate) sections: Vec<String>,
}

/// Collects the [`ShadowCensus`]: the profile chain's report, the boot
/// chain's report when a boot path is known, and the `.env` siblings.
pub(crate) fn shadow_census(
    leaf: &Path,
    boot: Option<&Path>,
) -> Result<ShadowCensus, GatewayError> {
    let profile = pending_report(leaf).map_err(|error| pending_read_error(&error))?;
    let mut files: Vec<PathBuf> = Vec::new();
    for file in &profile.shadowed_files {
        push_unique(&mut files, file);
    }
    let mut sections = profile.changed_sections;
    if let Some(boot) = boot {
        let report = pending_report(boot).map_err(|error| pending_read_error(&error))?;
        for file in &report.shadowed_files {
            push_unique(&mut files, file);
        }
        sections.extend(report.changed_sections);
    }
    // `.env` files never join a TOML chain; their shadows are checked
    // directly on the profile and boot siblings.
    let mut env_files = vec![leaf.with_extension("env")];
    if let Some(boot) = boot {
        env_files.push(boot.with_extension("env"));
    }
    for env in env_files {
        if shadow_path(&env).is_file() {
            push_unique(&mut files, &env);
        }
    }
    sections.sort_unstable();
    sections.dedup();
    Ok(ShadowCensus { files, sections })
}

/// The directory config files render relative to: the boot config's
/// parent, falling back to the profiles directory's parent when no boot
/// path is known.
pub(crate) fn config_root<'a>(leaf: &'a Path, boot: Option<&'a Path>) -> Option<&'a Path> {
    boot.and_then(Path::parent)
        .or_else(|| leaf.parent().and_then(Path::parent))
}

/// Assembles the `GET /admin/config-dirty` body: shadowed files from the
/// profile and boot chains plus the `.env` siblings, section diffs from
/// both chain comparisons.
fn dirty_reply(leaf: &Path, boot: Option<&Path>) -> Result<serde_json::Value, GatewayError> {
    let census = shadow_census(leaf, boot)?;
    let root = config_root(leaf, boot);
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
    let relative = root
        .map(canonical_form)
        .and_then(|root| file.strip_prefix(&root).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| file.to_path_buf());
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

#[cfg(test)]
mod tests {
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

    #[tokio::test]
    async fn with_no_shadows_pending_equals_config_and_dirty_is_clean() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;

        let running = get_json(addr, "admin/config").await;
        let pending = get_json(addr, "admin/config-pending").await;
        assert_eq!(
            pending["profile"], running,
            "no shadows: the pending view is the running view"
        );
        assert!(
            pending["boot"].is_null(),
            "no boot shadow: the boot side is null"
        );

        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(
            dirty,
            serde_json::json!({
                "dirty": false,
                "pending_files": [],
                "changed_sections": [],
            })
        );
    }

    #[tokio::test]
    async fn a_profile_shadow_drives_the_pending_view_and_the_dirty_report() {
        let (_temp, config, paths) = fixture();
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

        let pending = get_json(addr, "admin/config-pending").await;
        assert_eq!(
            pending["profile"]["model"][0]["description"], "edited in the UI",
            "the pending view carries the shadow's value"
        );
        let source = pending["profile"]["model"][0]["source_file"]
            .as_str()
            .expect("a source_file annotation");
        assert!(
            source.ends_with("main.toml.next"),
            "provenance names the shadow file: {source}"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["model"][0]["description"],
            "from the boot file",
            "the running view is untouched by a pending edit"
        );

        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty["dirty"], true);
        assert_eq!(
            dirty["pending_files"],
            serde_json::json!(["profiles/main.toml"]),
            "the shadow is named by its real file, relative to the config root"
        );
        assert_eq!(dirty["changed_sections"], serde_json::json!(["model"]));
    }

    #[tokio::test]
    async fn the_pending_view_carries_the_leaf_include_array_shadow_preferred() {
        // The chain editor reads its rows from this array; if the payload
        // dropped it or ignored a staged leaf shadow's reorder, the editor
        // would fall back to guessing membership and order.
        let (_temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        let pending = get_json(addr, "admin/config-pending").await;
        assert_eq!(
            pending["profile"]["include"],
            serde_json::json!(["../gateway.toml", "common.toml"]),
            "no shadow: the real leaf's include line, verbatim and ordered"
        );

        write_shadow(&leaf, "include = [\"common.toml\", \"../gateway.toml\"]\n")
            .expect("stage the reordered leaf shadow");
        let pending = get_json(addr, "admin/config-pending").await;
        assert_eq!(
            pending["profile"]["include"],
            serde_json::json!(["common.toml", "../gateway.toml"]),
            "a leaf shadow's include array outranks the real file's"
        );
        assert_eq!(
            get_json(addr, "admin/config").await["include"],
            serde_json::json!(["../gateway.toml", "common.toml"]),
            "the running view keeps the loaded include line"
        );
    }

    #[tokio::test]
    async fn a_boot_shadow_reports_distinctly_for_the_restart_banner() {
        let (temp, config, paths) = fixture();
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(
            &boot,
            &BOOT.replace("from the boot file", "edited boot entry"),
        )
        .expect("stage the boot shadow");

        let pending = get_json(addr, "admin/config-pending").await;
        let shadow = pending["boot"]["shadow"]
            .as_str()
            .expect("a boot shadow path");
        assert!(shadow.ends_with("gateway.toml.next"), "got: {shadow}");
        assert_eq!(
            pending["boot"]["changed_sections"],
            serde_json::json!(["model"]),
            "the boot side names the sections its shadow changes"
        );
        assert_eq!(
            pending["profile"]["model"][0]["description"], "edited boot entry",
            "the profile chain includes the boot file, so its shadow flows in"
        );
        let source = pending["profile"]["model"][0]["source_file"]
            .as_str()
            .expect("a source_file annotation");
        assert!(source.ends_with("gateway.toml.next"), "got: {source}");

        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty["dirty"], true);
        assert_eq!(
            dirty["pending_files"],
            serde_json::json!(["gateway.toml"]),
            "the boot file is listed once, though both chains reach it"
        );
        assert_eq!(dirty["changed_sections"], serde_json::json!(["model"]));
    }

    #[tokio::test]
    async fn env_shadows_count_as_dirty_pending_files() {
        let (temp, config, paths) = fixture();
        let profile_env = paths.profiles_dir.join("main.env");
        let boot_env = temp.path().join("gateway.env");
        let addr = serve_with_paths(config, paths).await;

        write_shadow(&profile_env, "HF_TOKEN=pending\n").expect("stage the profile env shadow");
        write_shadow(&boot_env, "BOOT_KEY=pending\n").expect("stage the boot env shadow");

        let dirty = get_json(addr, "admin/config-dirty").await;
        assert_eq!(dirty["dirty"], true);
        assert_eq!(
            dirty["pending_files"],
            serde_json::json!(["gateway.env", "profiles/main.env"]),
            "both env shadows are listed, though neither real file exists"
        );
        assert_eq!(
            dirty["changed_sections"],
            serde_json::json!([]),
            "env shadows change no TOML section"
        );
    }

    #[tokio::test]
    async fn secrets_in_the_pending_view_stay_redacted() {
        let (_temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        // The shadow on disk holds real secret material: a preserved value
        // and a literal replacement. None of it may reach the pending view.
        let mut body = get_json(addr, "admin/config").await;
        let endpoints = body["endpoint"].as_array_mut().expect("endpoints array");
        for entry in endpoints {
            if entry["id"] == "e2" {
                entry["api_key"] = serde_json::json!("new-literal-secret");
            }
        }
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let written = std::fs::read_to_string(shadow_path(&leaf)).expect("read shadow");
        assert!(
            written.contains("new-literal-secret"),
            "the shadow itself holds the literal: {written}"
        );

        let pending = get_json(addr, "admin/config-pending").await;
        assert_eq!(pending["profile"]["server"]["api_key"], "***");
        for entry in pending["profile"]["endpoint"]
            .as_array()
            .expect("endpoints array")
        {
            assert_eq!(entry["api_key"], "***", "endpoint {} leaks", entry["id"]);
        }
        let text = serde_json::to_string(&pending).expect("the reply serializes");
        assert!(
            !text.contains("new-literal-secret") && !text.contains("boot-endpoint-secret"),
            "no secret material anywhere in the payload: {text}"
        );
    }

    #[test]
    fn dirty_reply_without_a_boot_path_falls_back_to_the_profiles_parent_root() {
        // Embedded assemblies carry no boot config path: the boot side is
        // skipped and the root falls back to the profiles directory's
        // parent, so relative rendering must not regress to full paths.
        let (_temp, _config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        write_shadow(&leaf, MAIN).expect("stage the profile shadow");

        let reply = super::dirty_reply(&leaf, None).expect("the report assembles");
        assert_eq!(reply["dirty"], true);
        assert_eq!(
            reply["pending_files"],
            serde_json::json!(["profiles/main.toml"]),
            "the fallback root still renders the file relative"
        );
        assert_eq!(
            reply["changed_sections"],
            serde_json::json!([]),
            "a shadow identical to the real file changes no section"
        );
    }

    #[tokio::test]
    async fn pending_read_routes_require_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        for route in ["admin/config-pending", "admin/config-dirty"] {
            let response = http
                .get(format!("http://{addr}/{route}"))
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
