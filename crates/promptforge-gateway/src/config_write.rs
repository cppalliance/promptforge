//! Shadow-file write routes: `PUT /admin/config`, `PUT /admin/boot-config`,
//! and `PUT /admin/include/{path}`.
//!
//! Each route stages a pending TOML document beside its real file
//! (`default.toml` gains `default.toml.next`) without touching the real
//! file or reloading the gateway. The body is the config JSON shape
//! `GET /admin/config` returns; secrets arriving as `"***"` preserve the
//! existing value, and the merged pending configuration is validated
//! before any shadow is written, so a bad save leaves nothing behind. The
//! shadow mechanics live in `promptforge-gateway-config`; these handlers
//! own auth, path resolution, and the JSON-to-TOML boundary.

use std::path::{Component, Path, PathBuf};

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use promptforge_gateway_config::{
    ConfigErrorKind, save_boot_shadow, save_include_shadow, save_profile_shadow,
};

use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `PUT /admin/config` route: bearer-authed, stages the body as the
/// active profile leaf's shadow (`profiles/<name>.toml.next`).
///
/// The body is the full config JSON (the `GET /admin/config` shape;
/// provenance annotations are ignored). Redacted `"***"` secrets are
/// restored from the current pending chain, the merged result is validated
/// like a real load, and only then is the shadow written atomically. The
/// real files stay untouched and nothing reloads.
pub(crate) async fn admin_put_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Json(body) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock: apply promotes shadows without
    // re-validating, so the combination it promotes must be one the latest
    // save validated whole - saves serialize with apply, revert, and each
    // other.
    let _guard = state.apply.lock().await;
    let leaf = active_profile_path(&state).await?;
    let document = toml_document(body)?;
    let shadow = run_blocking(move || save_profile_shadow(&leaf, document)).await?;
    Ok(Json(shadow_reply(&shadow)))
}

/// The `PUT /admin/boot-config` route: bearer-authed, stages the body as
/// the boot config's shadow (`gateway.toml.next`).
///
/// Redacted secrets restore from the boot file's own current state. The
/// candidate must carry a well-formed `[server]` section, and when a
/// profile is active its merged chain is re-validated with the candidate
/// standing in for the boot file.
pub(crate) async fn admin_put_boot_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Json(body) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock; see `admin_put_config` for the why.
    let _guard = state.apply.lock().await;
    let boot = state
        .boot
        .config_path
        .clone()
        .ok_or(GatewayError::BootConfigUnavailable)?;
    let leaf = active_profile_path(&state).await.ok();
    let document = toml_document(body)?;
    let shadow = run_blocking(move || save_boot_shadow(&boot, leaf.as_deref(), document)).await?;
    Ok(Json(shadow_reply(&shadow)))
}

/// The `PUT /admin/include/{path}` route: bearer-authed, stages the body
/// as the shadow of one included file in the profile chain.
///
/// `{path}` is relative to the profiles directory and is the trust
/// boundary: absolute paths, drive prefixes, and any `..` or `.` component
/// are refused, the file must carry a `.toml` extension, and it must
/// already exist. The file must also be part of the active profile's
/// pending include chain - an out-of-chain target could never be
/// validated, so the save refuses it. Redacted secrets restore from that
/// file's own current state, and the active profile's merged chain must
/// validate with the candidate in place before the shadow is written.
pub(crate) async fn admin_put_include(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractors keeps auth first and puts rejections in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let UrlPath(raw) =
        raw.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let Json(body) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock; see `admin_put_config` for the why.
    let _guard = state.apply.lock().await;
    let leaf = active_profile_path(&state).await?;
    let target = include_target(crate::profiles_dir(&state)?, &raw)?;
    let document = toml_document(body)?;
    let shadow = run_blocking(move || save_include_shadow(&leaf, &target, document)).await?;
    Ok(Json(shadow_reply(&shadow)))
}

/// The active profile's leaf file: `<profiles_dir>/<active>.toml`.
pub(crate) async fn active_profile_path(state: &AppState) -> Result<PathBuf, GatewayError> {
    let dir = crate::profiles_dir(state)?.to_path_buf();
    let name = state
        .live
        .read()
        .await
        .profile_name
        .clone()
        .ok_or(GatewayError::ProfilesUnavailable)?;
    Ok(dir.join(format!("{name}.toml")))
}

/// Resolves and confines an include path: every component must be a plain
/// name (no root, prefix, `..`, or `.`), the extension must be `.toml`,
/// and the file must exist inside the profiles directory.
fn include_target(dir: &Path, raw: &str) -> Result<PathBuf, GatewayError> {
    let rel = Path::new(raw);
    let confined = !raw.is_empty()
        && rel
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !confined {
        return Err(GatewayError::MalformedRequest(format!(
            "include path must be relative with no traversal: {raw}"
        )));
    }
    if rel.extension().and_then(|ext| ext.to_str()) != Some("toml") {
        return Err(GatewayError::MalformedRequest(format!(
            "include path must name a .toml file: {raw}"
        )));
    }
    let target = dir.join(rel);
    if !target.is_file() {
        return Err(GatewayError::IncludeNotFound(raw.to_owned()));
    }
    Ok(target)
}

/// Runs one blocking shadow save on the blocking pool and maps its errors.
async fn run_blocking<F>(save: F) -> Result<PathBuf, GatewayError>
where
    F: FnOnce() -> Result<PathBuf, promptforge_gateway_config::ConfigError> + Send + 'static,
{
    tokio::task::spawn_blocking(save)
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
        .map_err(config_write_error)
}

/// The success body: the shadow path the save produced.
fn shadow_reply(shadow: &Path) -> serde_json::Value {
    serde_json::json!({ "shadow": shadow.display().to_string() })
}

/// Maps a config-crate failure onto the wire: a failed disk write is a
/// server fault (500), everything else - validation, parse, unresolved
/// `${VAR}`, an unreadable chain file - rejects the payload (422) with the
/// full cause chain so the UI can show why the save failed.
pub(crate) fn config_write_error(error: promptforge_gateway_config::ConfigError) -> GatewayError {
    if error.kind() == ConfigErrorKind::Write {
        GatewayError::ConfigWriteIo(Box::new(error))
    } else {
        GatewayError::ConfigWriteRejected(error_chain(&error))
    }
}

/// Renders an error and every source beneath it as one `; `-joined line.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str("; ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Converts the request body into the TOML document a shadow save takes:
/// provenance annotations from `GET /admin/config` are dropped, `null`
/// members are treated as absent, and the result must be a table.
fn toml_document(mut body: serde_json::Value) -> Result<toml::Value, GatewayError> {
    strip_provenance(&mut body);
    let value = json_to_toml(body)
        .map_err(GatewayError::ConfigWriteRejected)?
        .ok_or_else(|| {
            GatewayError::ConfigWriteRejected("the body must be a JSON object".to_owned())
        })?;
    if value.is_table() {
        Ok(value)
    } else {
        Err(GatewayError::ConfigWriteRejected(
            "the body must be a JSON object".to_owned(),
        ))
    }
}

/// Drops the annotations `GET /admin/config` adds: the top-level
/// `source_files` map and the `source_file` field on every keyed-array
/// entry. Any other unknown key is left for validation to reject.
fn strip_provenance(body: &mut serde_json::Value) {
    let Some(table) = body.as_object_mut() else {
        return;
    };
    table.remove("source_files");
    for array_name in ["dominion", "endpoint", "model", "local_model"] {
        let Some(entries) = table
            .get_mut(array_name)
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for entry in entries {
            if let Some(object) = entry.as_object_mut() {
                object.remove("source_file");
            }
        }
    }
}

/// Converts a JSON value into a TOML one. `None` means "absent": TOML has
/// no null, so a null object member simply drops out (the serializer skips
/// absent optionals on the way out, and the deserializer defaults them on
/// the way back in). A null inside an array has no such reading and is an
/// error, as is a number outside TOML's ranges.
fn json_to_toml(value: serde_json::Value) -> Result<Option<toml::Value>, String> {
    Ok(Some(match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Bool(flag) => toml::Value::Boolean(flag),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else if let Some(float) = number.as_f64() {
                toml::Value::Float(float)
            } else {
                return Err(format!("number {number} does not fit a TOML value"));
            }
        }
        serde_json::Value::String(text) => toml::Value::String(text),
        serde_json::Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                let Some(element) = json_to_toml(item)? else {
                    return Err("null inside an array has no TOML form".to_owned());
                };
                converted.push(element);
            }
            toml::Value::Array(converted)
        }
        serde_json::Value::Object(members) => {
            let mut table = toml::map::Map::new();
            for (key, member) in members {
                if let Some(converted) = json_to_toml(member)? {
                    table.insert(key, converted);
                }
            }
            toml::Value::Table(table)
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use promptforge_gateway_config::{Config, ProfileName, shadow_path};

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

    async fn get_config(addr: std::net::SocketAddr) -> serde_json::Value {
        reqwest::Client::new()
            .get(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends")
            .json()
            .await
            .expect("a JSON body")
    }

    #[tokio::test]
    async fn put_config_writes_the_leaf_shadow_and_touches_no_real_file() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;
        let before = real_files(temp.path());

        let mut body = get_config(addr).await;
        body["model"][0]["description"] = serde_json::json!("edited in the UI");
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let shadow = shadow_path(&leaf);
        assert!(shadow.is_file(), "the leaf shadow exists after the save");
        let written: toml::Value =
            toml::from_str(&std::fs::read_to_string(&shadow).expect("read shadow"))
                .expect("the shadow parses as TOML");
        assert_eq!(
            written["model"][0]["description"].as_str(),
            Some("edited in the UI")
        );

        for (path, bytes) in before {
            assert_eq!(
                std::fs::read_to_string(&path).expect("re-read fixture file"),
                bytes,
                "{} is byte-identical after the PUT",
                path.display()
            );
        }
        assert_eq!(
            get_config(addr).await["model"][0]["description"],
            "from the boot file",
            "the running config is not reloaded by a save"
        );
    }

    #[tokio::test]
    async fn put_config_rejects_an_invalid_merge_and_leaves_no_shadow() {
        let (_temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        let body = serde_json::json!({
            "server": { "bind": "127.0.0.1:0", "api_key": "***" },
            "model": [{
                "name": "bad",
                "description": "references a missing endpoint",
                "context": 1,
                "upstream": "u",
                "endpoints": ["ghost"],
            }],
        });
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        let text = response.text().await.expect("an error body");
        assert!(text.contains("ghost"), "the error names the cause: {text}");
        assert!(
            !shadow_path(&leaf).exists(),
            "a rejected save leaves no shadow behind"
        );
    }

    #[tokio::test]
    async fn secrets_round_trip_preserve_replace_and_reference() {
        let (temp, config, paths) = fixture();
        let leaf = paths.profiles_dir.join("main.toml");
        let addr = serve_with_paths(config, paths).await;

        // The ${VAR} reference must resolve during validation; load it into
        // the process env the same way the gateway loads env files at boot.
        let env_file = temp.path().join("s7-secret-fixture.env");
        std::fs::write(&env_file, "PFG_S7_KEY_REF=test-token\n").expect("write env fixture");
        crate::runner::load_env_file(&env_file);

        let mut body = get_config(addr).await;
        body["server"]["api_key"] = serde_json::json!("${PFG_S7_KEY_REF}");
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
            written.contains("boot-endpoint-secret"),
            "a \"***\" secret round-trips to the on-disk value: {written}"
        );
        assert!(
            written.contains("new-literal-secret"),
            "a literal secret replaces the old value: {written}"
        );
        assert!(
            written.contains("${PFG_S7_KEY_REF}"),
            "a ${{VAR}} reference is written as-is: {written}"
        );
        assert!(
            !written.contains("***"),
            "no redaction marker survives into the shadow: {written}"
        );
    }

    #[tokio::test]
    async fn put_boot_config_writes_the_boot_shadow_only() {
        let (temp, config, paths) = fixture();
        let boot = temp.path().join("gateway.toml");
        let addr = serve_with_paths(config, paths).await;
        let before = real_files(temp.path());

        let body = serde_json::json!({
            "server": { "bind": "127.0.0.1:0", "api_key": "***" },
            "endpoint": [{
                "id": "e",
                "protocol": "openai",
                "base_url": "http://127.0.0.1:9",
                "api_key": "***",
            }],
            "model": [{
                "name": "m",
                "description": "edited boot entry",
                "context": 2048,
                "upstream": "u",
                "endpoints": ["e"],
            }],
        });
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/boot-config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let written = std::fs::read_to_string(shadow_path(&boot)).expect("read boot shadow");
        assert!(
            written.contains("boot-endpoint-secret") && written.contains("test-token"),
            "boot secrets restore from the real boot file: {written}"
        );
        for (path, bytes) in before {
            assert_eq!(
                std::fs::read_to_string(&path).expect("re-read fixture file"),
                bytes,
                "{} is byte-identical after the PUT",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn put_include_writes_the_include_shadow() {
        let (temp, config, paths) = fixture();
        let common = paths.profiles_dir.join("common.toml");
        let addr = serve_with_paths(config, paths).await;

        let body = serde_json::json!({
            "endpoint": [{
                "id": "e2",
                "protocol": "openai",
                "base_url": "http://127.0.0.1:10",
                "api_key": "***",
            }],
        });
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/include/common.toml"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let written = std::fs::read_to_string(shadow_path(&common)).expect("read shadow");
        assert!(
            written.contains("common-endpoint-secret"),
            "the include's own file supplies the preserved secret: {written}"
        );
        assert_eq!(
            std::fs::read_to_string(&common).expect("re-read common"),
            COMMON,
            "the real include file is untouched"
        );
        drop(temp);
    }

    #[tokio::test]
    async fn put_include_rejects_traversal_absolute_and_non_toml_paths() {
        let (temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        let body = serde_json::json!({});

        // Encoded so the URL parser cannot collapse the dot-segments before
        // the handler sees them.
        for encoded in [
            "%2e%2e%2fgateway.toml", // ../gateway.toml
            "..%2Fgateway.toml",     // ../gateway.toml
            "%2Fabs.toml",           // /abs.toml (rooted)
            "common.toml.next",      // wrong extension: shadows are not targets
        ] {
            let response = http
                .put(format!("http://{addr}/admin/include/{encoded}"))
                .bearer_auth("test-token")
                .json(&body)
                .send()
                .await
                .expect("the request sends");
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "{encoded} must be refused at the trust boundary"
            );
        }

        // A drive-prefixed absolute path: a prefix component on Windows, an
        // unknown file elsewhere; refused either way.
        let response = http
            .put(format!(
                "http://{addr}/admin/include/C%3A%5Cboot%5Cgateway.toml"
            ))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert!(
            response.status().is_client_error(),
            "a drive-prefixed path is refused"
        );

        let missing = http
            .put(format!("http://{addr}/admin/include/ghost.toml"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

        let mut shadows = Vec::new();
        for dir in [temp.path().to_path_buf(), temp.path().join("profiles")] {
            for entry in std::fs::read_dir(dir).expect("read dir") {
                let name = entry.expect("dir entry").file_name();
                if name.to_string_lossy().ends_with(".next") {
                    shadows.push(name);
                }
            }
        }
        assert!(
            shadows.is_empty(),
            "no refused request may write a shadow: {shadows:?}"
        );
    }

    #[tokio::test]
    async fn put_include_refuses_a_file_outside_the_active_chain() {
        let (_temp, config, paths) = fixture();
        let stray = paths.profiles_dir.join("stray.toml");
        std::fs::write(&stray, "").expect("write stray file");
        let addr = serve_with_paths(config, paths).await;

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/include/stray.toml"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        let text = response.text().await.expect("an error body");
        assert!(
            text.contains("pending include chain"),
            "the refusal names the reason: {text}"
        );
        assert!(
            !shadow_path(&stray).exists(),
            "no unvalidated shadow is written for an out-of-chain file"
        );
    }

    #[tokio::test]
    async fn malformed_bodies_map_into_the_error_envelope_after_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();

        for route in [
            "admin/config",
            "admin/boot-config",
            "admin/include/common.toml",
        ] {
            let unauthed = http
                .put(format!("http://{addr}/{route}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body("{not json")
                .send()
                .await
                .expect("the request sends");
            assert_eq!(
                unauthed.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "{route}: auth wins over a malformed body"
            );

            let authed = http
                .put(format!("http://{addr}/{route}"))
                .bearer_auth("test-token")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body("{not json")
                .send()
                .await
                .expect("the request sends");
            assert_eq!(authed.status(), reqwest::StatusCode::BAD_REQUEST);
            let body: serde_json::Value = authed.json().await.expect("a JSON envelope");
            assert_eq!(
                body["error"]["code"], "malformed_request",
                "{route}: the rejection lands in the gateway envelope"
            );
        }
    }

    #[tokio::test]
    async fn shadow_write_routes_require_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();
        for route in [
            "admin/config",
            "admin/boot-config",
            "admin/include/common.toml",
        ] {
            let response = http
                .put(format!("http://{addr}/{route}"))
                .json(&serde_json::json!({}))
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
