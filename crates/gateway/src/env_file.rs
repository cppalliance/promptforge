//! The `GET /admin/env` and `PUT /admin/env` routes: read the single `.env`
//! file and stage edits as an `.env.next` shadow.
//!
//! The gateway loads only the config sibling (`gateway.env`) at boot. `GET`
//! returns it parsed, values included - the caller already presented the
//! gateway's own bearer key, and `build_router` puts these routes behind
//! the shared loopback wall in every build. `PUT` writes its shadow
//! atomically. The real file stays untouched until Apply, and the process
//! environment changes only on restart.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use gateway_config::{pending_var_references, write_shadow};

use crate::auth::Caller;
use crate::config_write::config_write_error;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `GET /admin/env` route: bearer-authed, parses the global `.env` file.
///
/// The reply is `{"boot": section, "profile": section, "references": map}`
/// where `boot` is `{"path", "vars"}`, `profile` is `null`, and `references`
/// maps each `${VAR}` name the pending config references to labels
/// of the referencing fields (`endpoint openai api_key`). The references
/// come from the raw pre-interpolation chain because a loaded config
/// interpolates every reference away and redacts secrets - the UI's
/// "used by" annotations are computable only server-side. A missing file
/// is an empty `vars` map.
pub(crate) async fn admin_get_env(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let config = crate::config_path(&state)?.to_path_buf();
    let env = config.with_extension("env");
    let reply = tokio::task::spawn_blocking(move || {
        // The reference scan parses the pending document without validating
        // or interpolating it, so a failure means an unreadable or
        // unparsable config file - surfaced, never hidden.
        let references = pending_var_references(&config)
            .map_err(|error| GatewayError::EnvFile(Box::new(error)))?;
        Ok::<_, GatewayError>(serde_json::json!({
            "boot": env_section(Some(&env))?,
            "profile": null,
            "references": references,
        }))
    })
    .await
    .map_err(|join| GatewayError::EnvFile(Box::new(join)))??;
    Ok(Json(reply))
}

/// The `PUT /admin/env` query: which env file's shadow the body targets.
#[derive(serde::Deserialize)]
pub(crate) struct EnvPutQuery {
    /// `"global"` when present; absent targets the same global file.
    scope: Option<String>,
}

/// The `PUT /admin/env` route: bearer-authed, writes the global env shadow.
///
/// The body is a flat JSON object of variable names to values. Names must
/// be `[A-Za-z_][A-Za-z0-9_]*`; values are rendered bare, single-quoted,
/// or double-quoted so they round-trip through the same dotenv parser the
/// gateway boots with, and a value no quoting can carry (an embedded
/// newline, or a single quote mixed with `$`, `"`, or `\`) is refused.
/// The real `.env` file is never touched.
pub(crate) async fn admin_put_env(
    State(state): State<AppState>,
    caller: Caller,
    scope: Result<Query<EnvPutQuery>, QueryRejection>,
    vars: Result<Json<BTreeMap<String, String>>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    // Deferring the extractors keeps auth first and puts rejections in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Query(scope) =
        scope.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let Json(vars) =
        vars.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock; see `admin_put_config` for the why.
    let _guard = state.apply.lock().await;
    let env = match scope.scope.as_deref() {
        None | Some("global") => crate::config_path(&state)?.with_extension("env"),
        Some(other) => {
            return Err(GatewayError::ConfigWriteRejected(format!(
                "unknown env scope {other:?}: use \"global\""
            )));
        }
    };
    let contents = render_env(&vars)?;
    let shadow = tokio::task::spawn_blocking(move || write_shadow(&env, &contents))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
        .map_err(config_write_error)?;
    Ok(Json(
        serde_json::json!({ "shadow": shadow.display().to_string() }),
    ))
}

/// One side of the `GET /admin/env` reply: the file's path and its parsed
/// variables, or `null` when that side is not configured.
fn env_section(path: Option<&Path>) -> Result<serde_json::Value, GatewayError> {
    let Some(path) = path else {
        return Ok(serde_json::Value::Null);
    };
    Ok(serde_json::json!({
        "path": path.display().to_string(),
        "vars": parse_env(path)?,
    }))
}

/// Parses one `.env` file into a map, without touching the process
/// environment. A missing file is an empty map.
fn parse_env(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, GatewayError> {
    let mut vars = serde_json::Map::new();
    if !path.is_file() {
        return Ok(vars);
    }
    let entries =
        dotenvy::from_path_iter(path).map_err(|error| GatewayError::EnvFile(Box::new(error)))?;
    for entry in entries {
        let (key, value) = entry.map_err(|error| GatewayError::EnvFile(Box::new(error)))?;
        vars.insert(key, serde_json::Value::from(value));
    }
    Ok(vars)
}

/// Renders the variables as dotenv lines, refusing names and values the
/// boot-time parser could not round-trip.
fn render_env(vars: &BTreeMap<String, String>) -> Result<String, GatewayError> {
    let mut out = String::new();
    for (key, value) in vars {
        if !valid_key(key) {
            return Err(GatewayError::ConfigWriteRejected(format!(
                "invalid env variable name {key:?}: use letters, digits, and underscores, \
                 not starting with a digit"
            )));
        }
        let rendered = render_value(value).ok_or_else(|| {
            GatewayError::ConfigWriteRejected(format!(
                "env variable {key} has a value no dotenv quoting can carry \
                 (an embedded newline, or a single quote mixed with $, \", or \\)"
            ))
        })?;
        let _infallible = writeln!(out, "{key}={rendered}");
    }
    Ok(out)
}

/// Whether `key` is a dotenv-safe variable name.
fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Renders one value so dotenv parsing returns it verbatim: bare when every
/// character is inert, single-quoted (literal, no substitution) otherwise,
/// double-quoted when the value itself holds single quotes but none of the
/// characters double quoting gives meaning to. `None` when no form works.
fn render_value(value: &str) -> Option<String> {
    if value.chars().all(bare_safe) {
        return Some(value.to_owned());
    }
    if value.contains(['\n', '\r', '\0']) {
        return None;
    }
    if !value.contains('\'') {
        return Some(format!("'{value}'"));
    }
    if !value.contains(['"', '\\', '$']) {
        return Some(format!("\"{value}\""));
    }
    None
}

/// Characters safe to write unquoted: no whitespace, no comment marker, no
/// quotes, no `$` substitution, no `=`.
fn bare_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_@%+:,./-".contains(c)
}

#[cfg(test)]
mod tests {
    use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path};

    use super::render_value;
    use crate::test_support::{AdminPaths, serve_with_paths};

    const CONFIG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[profile]]
name = "main"
models = []
"#;

    fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, CONFIG).expect("write config");
        std::fs::write(
            profile_state_path(&config_path),
            "active_profile = \"main\"\n",
        )
        .expect("write state");
        std::fs::write(config_path.with_extension("env"), "GLOBAL=one\n").expect("write env");
        let config = Config::load(&config_path, &ProfileSelection::default()).expect("load config");
        let paths = AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "main".to_owned(),
            config_path,
        };
        (temp, config, paths)
    }

    #[tokio::test]
    async fn env_routes_expose_only_the_single_global_file() {
        let (_temp, config, paths) = fixture();
        let env_path = paths.config_path.with_extension("env");
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();

        let get: serde_json::Value = http
            .get(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("get sends")
            .json()
            .await
            .expect("env json");
        assert_eq!(get["boot"]["vars"], serde_json::json!({"GLOBAL": "one"}));
        assert!(get["profile"].is_null());

        let put = http
            .put(format!("http://{addr}/admin/env?scope=global"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({"GLOBAL": "two"}))
            .send()
            .await
            .expect("put sends");
        assert_eq!(put.status(), reqwest::StatusCode::OK);
        assert!(shadow_path(&env_path).is_file());

        let profile = http
            .put(format!("http://{addr}/admin/env?scope=profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("profile put sends");
        assert_eq!(profile.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn render_value_round_trips_supported_forms() {
        assert_eq!(render_value("abc-123"), Some("abc-123".to_owned()));
        assert_eq!(render_value("two words"), Some("'two words'".to_owned()));
        assert_eq!(render_value("it's"), Some("\"it's\"".to_owned()));
        assert_eq!(render_value("a\nb"), None);
    }
}
