//! Shadow-file write route: `PUT /admin/config`.
//!
//! The route stages the pending global TOML document beside its real file
//! (`gateway.toml` gains `gateway.toml.next`) without touching the real
//! file or reloading the gateway. The body is the config JSON shape
//! `GET /admin/config` returns; secrets arriving as `"***"` preserve the
//! existing value, and the merged pending configuration is validated
//! before any shadow is written, so a bad save leaves nothing behind. The
//! shadow mechanics live in `gateway-config`; these handlers
//! own auth, path resolution, and the JSON-to-TOML boundary.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use gateway_config::{ConfigErrorKind, save_config_shadow};

use crate::auth::Caller;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `PUT /admin/config` route: bearer-authed, stages the global config
/// and optional sibling profile state.
///
/// The body is the full `GET /admin/config` JSON shape. Redacted `"***"` secrets are
/// restored from the current pending chain, the merged result is validated
/// like a real load, and only then is the shadow written atomically. The
/// real files stay untouched and nothing reloads.
pub(crate) async fn admin_put_config(
    State(state): State<AppState>,
    caller: Caller,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Json(body) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock: apply promotes shadows without
    // re-validating, so the combination it promotes must be one the latest
    // save validated whole - saves serialize with apply, revert, and each
    // other.
    let _guard = state.apply.lock().await;
    let config = crate::config_path(&state)?.to_path_buf();
    let document = toml_document(body)?;
    let shadows = tokio::task::spawn_blocking(move || save_config_shadow(&config, document))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
        .map_err(config_write_error)?;
    Ok(Json(serde_json::json!({
        "shadow": shadows.config.display().to_string(),
        "state_shadow": shadows.state.map(|path| path.display().to_string()),
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
    use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path};

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

    #[tokio::test]
    async fn pending_active_profile_does_not_switch_before_apply() {
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

        let response = http
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("put sends");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(shadow_path(&config_path).is_file());
        assert!(shadow_path(&profile_state_path(&config_path)).is_file());
        let status: serde_json::Value = http
            .get(format!("http://{addr}/admin/status"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("status sends")
            .json()
            .await
            .expect("status json");
        assert_eq!(status["profile"], "alpha");
        assert_eq!(status["models"], serde_json::json!(["alpha-model"]));
    }
}
