//! The `/admin/config` routes: `GET` renders the running global
//! configuration as JSON with secrets redacted; `PUT` stages the pending
//! global TOML document as a shadow file.
//!
//! The write stages the document beside its real file (`gateway.toml`
//! gains `gateway.toml.next`) without touching the real file or reloading
//! the gateway. The body is the config JSON shape `GET /admin/config`
//! returns; secrets arriving as `"***"` preserve the existing value, and
//! the merged pending configuration is validated before any shadow is
//! written, so a bad save leaves nothing behind. The shadow mechanics live
//! in `gateway-config`; these handlers own auth, path resolution, and the
//! JSON-to-TOML boundary.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use gateway_config::{ConfigErrorKind, save_config_shadow};

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, WireJson, blocking};

/// The `/admin/config` routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/admin/config", get(admin_config).put(admin_put_config))
}

/// The `GET /admin/config` route: bearer-authed, renders the running global
/// config in the pending admin shape. The running profile is not part of
/// the document (`GET /admin/status` reports it), so the reply round-trips
/// through `PUT /admin/config` unchanged.
pub(crate) async fn admin_config(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let config = state.config().await;
    Ok(Json(config.to_json()))
}

/// The `PUT /admin/config` route: bearer-authed, stages the global config.
///
/// The body is the full `GET /admin/config` JSON shape. Redacted `"***"`
/// secrets are restored from the current pending chain, the merged result
/// is validated like a real load, and only then is the shadow written
/// atomically. The real file stays untouched and nothing reloads. The reply
/// is `{"shadow": path}`. A body carrying `active_profile` is rejected as a
/// config-write error: selection belongs to `POST /admin/switch-profile`.
pub(crate) async fn admin_put_config(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
    WireJson(body): WireJson<serde_json::Value>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    // Saves take the apply lock: apply promotes shadows without
    // re-validating, so the combination it promotes must be one the latest
    // save validated whole - saves serialize with apply, revert, and each
    // other.
    let _guard = state.apply.lock().await;
    let config = crate::admin::config_path(&state)?.to_path_buf();
    let document = toml_document(body)?;
    let shadows = blocking(move || save_config_shadow(&config, document))
        .await?
        .map_err(config_write_error)?;
    Ok(Json(serde_json::json!({
        "shadow": shadows.config.display().to_string(),
    })))
}

/// Maps a config-crate failure onto the wire: a failed disk write is a
/// server fault (500), everything else - validation, parse, unresolved
/// `${VAR}`, an unreadable chain file - rejects the payload (422) with the
/// full cause chain so the UI can show why the save failed.
pub(crate) fn config_write_error(error: gateway_config::ConfigError) -> GatewayError {
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

/// Converts the request body into the TOML document a shadow save takes.
fn toml_document(body: serde_json::Value) -> Result<toml::Value, GatewayError> {
    let value = json_to_toml(body)?.ok_or_else(|| {
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

/// Converts a JSON value into a TOML one. `None` means "absent": TOML has
/// no null, so a null object member simply drops out (the serializer skips
/// absent optionals on the way out, and the deserializer defaults them on
/// the way back in). A null inside an array has no such reading and is an
/// error, as is a number outside TOML's ranges.
fn json_to_toml(value: serde_json::Value) -> Result<Option<toml::Value>, GatewayError> {
    Ok(Some(match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Bool(flag) => toml::Value::Boolean(flag),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else if let Some(float) = number.as_f64() {
                toml::Value::Float(float)
            } else {
                return Err(GatewayError::ConfigWriteRejected(format!(
                    "number {number} does not fit a TOML value"
                )));
            }
        }
        serde_json::Value::String(text) => toml::Value::String(text),
        serde_json::Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                let Some(element) = json_to_toml(item)? else {
                    return Err(GatewayError::ConfigWriteRejected(
                        "null inside an array has no TOML form".to_owned(),
                    ));
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
    use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path};

    use crate::test_support::{AdminPaths, serve_with_paths};

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

    /// The live config as a save body: `GET /admin/config` also reports the
    /// running `active_profile`, which is not a configuration key and never
    /// goes back in a save.
    async fn save_body(addr: std::net::SocketAddr) -> serde_json::Value {
        let mut body: serde_json::Value = reqwest::Client::new()
            .get(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("get sends")
            .json()
            .await
            .expect("config json");
        body.as_object_mut()
            .expect("the config is an object")
            .remove("active_profile");
        body
    }

    async fn put_config(addr: std::net::SocketAddr, body: &serde_json::Value) -> reqwest::Response {
        reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(body)
            .send()
            .await
            .expect("put sends")
    }

    #[tokio::test]
    async fn a_save_carrying_active_profile_is_rejected_and_stages_nothing() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let addr = serve_with_paths(config, paths).await;
        let mut body = save_body(addr).await;
        body["active_profile"] = serde_json::json!("beta");

        let response = put_config(addr, &body).await;

        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        let error: serde_json::Value = response.json().await.expect("error envelope");
        assert_eq!(error["error"]["code"], "config_write_rejected");
        assert!(
            error["error"]["message"].as_str().is_some_and(|message| {
                message.contains("active_profile is not a configuration key")
                    && message.contains("POST /admin/switch-profile")
            }),
            "the message names the switch route: {error}"
        );
        assert!(!shadow_path(&config_path).exists());
        assert!(!shadow_path(&profile_state_path(&config_path)).exists());
    }

    #[tokio::test]
    async fn a_save_replies_with_the_config_shadow_alone() {
        let (_temp, config, paths) = fixture();
        let config_path = paths.config_path.clone();
        let addr = serve_with_paths(config, paths).await;
        let mut body = save_body(addr).await;
        body["model"][0]["description"] = serde_json::json!("edited");

        let response = put_config(addr, &body).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = response.json().await.expect("save reply");
        assert_eq!(
            reply,
            serde_json::json!({ "shadow": shadow_path(&config_path).display().to_string() }),
            "the reply carries only the config shadow"
        );
        assert!(shadow_path(&config_path).is_file());
        assert!(!shadow_path(&profile_state_path(&config_path)).exists());
    }
}
