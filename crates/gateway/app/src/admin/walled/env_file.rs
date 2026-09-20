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

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use gateway_config::{pending_var_references, write_shadow};
use serde::Serialize;

use super::config::{ShadowReply, config_write_error};
use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, WireJson, WireQuery, blocking};
use crate::registry::RouteInfo;

/// The `GET /admin/env` reply.
#[derive(Debug, Serialize)]
pub(crate) struct EnvReply {
    /// The config-sibling `.env` file the gateway boots with.
    boot: Option<EnvSection>,
    /// Always `null`: profiles carry no env file of their own.
    profile: Option<EnvSection>,
    /// Each `${VAR}` name the pending config references, mapped to labels
    /// of the referencing fields.
    references: BTreeMap<String, Vec<String>>,
}

/// One side of the `GET /admin/env` reply: an env file's path and its
/// parsed variables.
#[derive(Debug, Serialize)]
pub(crate) struct EnvSection {
    path: String,
    vars: serde_json::Map<String, serde_json::Value>,
}

const ENV: RouteInfo = RouteInfo::walled("/admin/env", &[Method::GET, Method::PUT]);

/// The `/admin/env` routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[ENV];

/// The `/admin/env` routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(ENV.path, get(admin_get_env).put(admin_put_env))
}

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
    _caller: LoopbackCaller,
) -> Result<Json<EnvReply>, GatewayError> {
    let config = crate::admin::config_path(&state)?.to_path_buf();
    let env = config.with_extension("env");
    let reply = blocking(move || {
        // The reference scan parses the pending document without validating
        // or interpolating it, so a failure means an unreadable or
        // unparsable config file - surfaced, never hidden.
        let references = pending_var_references(&config)
            .map_err(|error| GatewayError::EnvFile(Box::new(error)))?;
        Ok::<_, GatewayError>(EnvReply {
            boot: Some(env_section(&env)?),
            profile: None,
            references,
        })
    })
    .await??;
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
    _caller: LoopbackCaller,
    WireQuery(scope): WireQuery<EnvPutQuery>,
    WireJson(vars): WireJson<BTreeMap<String, String>>,
) -> Result<Json<ShadowReply>, GatewayError> {
    // Saves take the apply lock; see `admin_put_config` for the why.
    let _guard = state.apply.lock().await;
    let env = match scope.scope.as_deref() {
        None | Some("global") => crate::admin::config_path(&state)?.with_extension("env"),
        Some(other) => {
            return Err(GatewayError::ConfigWriteRejected(format!(
                "unknown env scope {other:?}: use \"global\""
            )));
        }
    };
    let contents = render_env(&vars)?;
    let shadow = blocking(move || write_shadow(&env, &contents))
        .await?
        .map_err(config_write_error)?;
    Ok(Json(ShadowReply::staged(&shadow)))
}

/// One side of the `GET /admin/env` reply: the file's path and its parsed
/// variables.
fn env_section(path: &Path) -> Result<EnvSection, GatewayError> {
    Ok(EnvSection {
        path: path.display().to_string(),
        vars: parse_env(path)?,
    })
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
#[path = "env_file-tests.rs"]
mod tests;
