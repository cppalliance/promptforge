//! The `GET /admin/env` and `PUT /admin/env` routes: read the real `.env`
//! files, stage edits as an `.env.next` shadow.
//!
//! The gateway loads two env files at boot: the boot config's sibling
//! (`gateway.env`) and the active profile's (`<profile>.env`). `GET`
//! returns both parsed, values included - the caller already presented the
//! gateway's own bearer key, and `build_router` puts these routes behind
//! the shared loopback wall in every build. `PUT` writes one env file's
//! shadow atomically - the active profile's (`<profile>.env.next`) by default,
//! the boot config's (`gateway.env.next`) with `?scope=boot`; the real
//! file is untouched and the process environment does not change until an
//! explicit apply and restart or profile switch.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use promptforge_gateway_config::{pending_var_references, write_shadow};

use crate::config_write::{active_profile_path, config_write_error};
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `GET /admin/env` route: bearer-authed, parses the real boot and
/// profile `.env` files and returns their variables with values included.
///
/// The reply is `{"boot": section, "profile": section, "references": map}`
/// where a section is `{"path", "vars"}` or `null` when that side is not
/// configured (no boot path, no profiles directory), and `references`
/// maps each `${VAR}` name the pending config chain references to labels
/// of the referencing fields (`endpoint openai api_key`). The references
/// come from the raw pre-interpolation chain because a loaded config
/// interpolates every reference away and redacts secrets - the UI's
/// "used by" annotations are computable only server-side. A missing file
/// is an empty `vars` map.
pub(crate) async fn admin_get_env(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let boot = state
        .boot
        .config_path
        .as_ref()
        .map(|path| path.with_extension("env"));
    let leaf = active_profile_path(&state).await.ok();
    let profile = leaf.as_ref().map(|leaf| leaf.with_extension("env"));
    let reply = tokio::task::spawn_blocking(move || {
        // The reference scan parses the pending chain without validating
        // or interpolating it, so a failure means an unreadable or
        // unparsable config file - surfaced, never hidden.
        let references = leaf
            .as_deref()
            .map(pending_var_references)
            .transpose()
            .map_err(|error| GatewayError::EnvFile(Box::new(error)))?
            .unwrap_or_default();
        Ok::<_, GatewayError>(serde_json::json!({
            "boot": env_section(boot.as_deref())?,
            "profile": env_section(profile.as_deref())?,
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
    /// `"profile"` (the default when absent) or `"boot"`.
    scope: Option<String>,
}

/// The `PUT /admin/env` route: bearer-authed, writes the body's key-value
/// pairs as one env file's shadow - the active profile's
/// (`<profile>.env.next`) by default, the boot config's
/// (`gateway.env.next`) with `?scope=boot`.
///
/// The body is a flat JSON object of variable names to values. Names must
/// be `[A-Za-z_][A-Za-z0-9_]*`; values are rendered bare, single-quoted,
/// or double-quoted so they round-trip through the same dotenv parser the
/// gateway boots with, and a value no quoting can carry (an embedded
/// newline, or a single quote mixed with `$`, `"`, or `\`) is refused.
/// The real `.env` file is never touched.
pub(crate) async fn admin_put_env(
    State(state): State<AppState>,
    headers: HeaderMap,
    scope: Result<Query<EnvPutQuery>, QueryRejection>,
    vars: Result<Json<BTreeMap<String, String>>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractors keeps auth first and puts rejections in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Query(scope) =
        scope.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let Json(vars) =
        vars.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    // Saves take the apply lock; see `admin_put_config` for the why.
    let _guard = state.apply.lock().await;
    let env = match scope.scope.as_deref() {
        None | Some("profile") => profile_env_path(&state)
            .await
            .ok_or(GatewayError::ProfilesUnavailable)?,
        Some("boot") => state
            .boot
            .config_path
            .as_ref()
            .map(|path| path.with_extension("env"))
            .ok_or(GatewayError::BootConfigUnavailable)?,
        Some(other) => {
            return Err(GatewayError::ConfigWriteRejected(format!(
                "unknown env scope {other:?}: use \"profile\" or \"boot\""
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

/// The active profile's env file (`<profiles_dir>/<active>.env`), when a
/// profiles directory and an active profile exist.
async fn profile_env_path(state: &AppState) -> Option<PathBuf> {
    let leaf = active_profile_path(state).await.ok()?;
    Some(leaf.with_extension("env"))
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
    use std::collections::BTreeMap;

    use promptforge_gateway_config::{Config, ProfileName, shadow_path};

    use crate::test_support::{AdminPaths, serve_with_paths};

    const BOOT: &str = r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"
"#;

    /// A tempdir with a boot catalog, a `main` profile including it, and
    /// fixture `.env` files on both sides.
    fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(temp.path().join("gateway.toml"), BOOT).expect("write boot");
        std::fs::write(temp.path().join("gateway.env"), "BOOT_ONLY=from-boot\n")
            .expect("write boot env");
        let profiles = temp.path().join("profiles");
        std::fs::create_dir(&profiles).expect("mkdir profiles");
        std::fs::write(
            profiles.join("main.toml"),
            "include = [\"../gateway.toml\"]\n",
        )
        .expect("write main");
        std::fs::write(
            profiles.join("main.env"),
            "HF_TOKEN=hf-secret\nOPENAI_API_KEY=sk-123\n",
        )
        .expect("write profile env");
        let config = Config::load_profile(&profiles, &ProfileName::parse("main").expect("name"))
            .expect("the fixture profile loads");
        let paths = AdminPaths {
            profiles_dir: profiles,
            active: "main".to_owned(),
            boot_config: temp.path().join("gateway.toml"),
        };
        (temp, config, paths)
    }

    #[tokio::test]
    async fn get_env_returns_both_files_parsed_with_values() {
        let (_temp, config, paths) = fixture();
        // A staged leaf shadow carries a raw `${VAR}` reference: the scan
        // reads the pre-interpolation pending chain, the only place a
        // reference survives (a loaded config interpolates it away).
        promptforge_gateway_config::write_shadow(
            &paths.profiles_dir.join("main.toml"),
            concat!(
                "include = [\"../gateway.toml\"]\n",
                "[[endpoint]]\n",
                "id = \"openai\"\n",
                "protocol = \"openai\"\n",
                "base_url = \"https://api.openai.example/v1\"\n",
                "api_key = \"${OPENAI_API_KEY}\"\n",
            ),
        )
        .expect("stage the leaf shadow");
        let addr = serve_with_paths(config, paths).await;

        let body: serde_json::Value = reqwest::Client::new()
            .get(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends")
            .json()
            .await
            .expect("a JSON body");

        assert_eq!(
            body["boot"]["vars"],
            serde_json::json!({ "BOOT_ONLY": "from-boot" })
        );
        assert_eq!(
            body["profile"]["vars"],
            serde_json::json!({ "HF_TOKEN": "hf-secret", "OPENAI_API_KEY": "sk-123" })
        );
        assert!(
            body["boot"]["path"]
                .as_str()
                .expect("a boot path")
                .ends_with("gateway.env")
        );
        assert!(
            body["profile"]["path"]
                .as_str()
                .expect("a profile path")
                .ends_with("main.env")
        );
        assert_eq!(
            body["references"],
            serde_json::json!({ "OPENAI_API_KEY": ["endpoint openai api_key"] }),
            "the pending chain's ${{VAR}} references label their fields by entry identity"
        );
    }

    #[tokio::test]
    async fn put_env_writes_a_shadow_that_round_trips_through_dotenv() {
        let (_temp, config, paths) = fixture();
        let env = paths.profiles_dir.join("main.env");
        let addr = serve_with_paths(config, paths).await;

        let mut sent = BTreeMap::new();
        sent.insert("PLAIN".to_owned(), "abc-123".to_owned());
        sent.insert("SPACED".to_owned(), "two words #tail".to_owned());
        sent.insert("QUOTED".to_owned(), "it's quoted".to_owned());
        sent.insert("EMPTY".to_owned(), String::new());
        // Single quotes must carry `$` literally: a substituting parse
        // would come back changed and fail the equality below.
        sent.insert("DOLLAR".to_owned(), "pa$$word".to_owned());
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .json(&sent)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let shadow = shadow_path(&env);
        assert!(
            shadow.ends_with("main.env.next"),
            "got: {}",
            shadow.display()
        );
        let parsed: BTreeMap<String, String> = dotenvy::from_path_iter(&shadow)
            .expect("the shadow parses as dotenv")
            .collect::<Result<_, _>>()
            .expect("every line parses");
        assert_eq!(
            parsed, sent,
            "the shadow round-trips through dotenv parsing"
        );
        assert_eq!(
            std::fs::read_to_string(&env).expect("re-read the real env"),
            "HF_TOKEN=hf-secret\nOPENAI_API_KEY=sk-123\n",
            "the real .env file is byte-identical after the PUT"
        );

        let bad_key = reqwest::Client::new()
            .put(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "9BAD": "x" }))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            bad_key.status(),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "an invalid variable name is refused"
        );

        let bad_value = reqwest::Client::new()
            .put(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "SNEAKY": "a\nINJECTED=b" }))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            bad_value.status(),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "a value no quoting can carry is refused, never written"
        );
        assert!(
            !shadow_path(&env).exists() || {
                let body = std::fs::read_to_string(shadow_path(&env)).expect("read shadow");
                !body.contains("INJECTED")
            },
            "a refused value never reaches the shadow"
        );
    }

    #[tokio::test]
    async fn put_env_with_boot_scope_writes_the_boot_env_shadow() {
        let (temp, config, paths) = fixture();
        let boot_env = temp.path().join("gateway.env");
        let profile_env = paths.profiles_dir.join("main.env");
        let addr = serve_with_paths(config, paths).await;

        let mut sent = BTreeMap::new();
        sent.insert("BOOT_ONLY".to_owned(), "staged".to_owned());
        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/env?scope=boot"))
            .bearer_auth("test-token")
            .json(&sent)
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let shadow = shadow_path(&boot_env);
        assert!(
            shadow.ends_with("gateway.env.next"),
            "got: {}",
            shadow.display()
        );
        let parsed: BTreeMap<String, String> = dotenvy::from_path_iter(&shadow)
            .expect("the boot shadow parses as dotenv")
            .collect::<Result<_, _>>()
            .expect("every line parses");
        assert_eq!(parsed, sent, "the boot shadow carries the staged variables");
        assert_eq!(
            std::fs::read_to_string(&boot_env).expect("re-read the real boot env"),
            "BOOT_ONLY=from-boot\n",
            "the real boot .env file is byte-identical after the PUT"
        );
        assert!(
            !shadow_path(&profile_env).exists(),
            "a boot-scoped PUT never touches the profile side"
        );
    }

    #[tokio::test]
    async fn get_env_surfaces_an_unparsable_chain_file_instead_of_hiding_it() {
        // `write_shadow` stages arbitrary bytes, so a tampered shadow is
        // the one way an unparsable chain file can exist; the reference
        // scan must surface it, never silently return no references.
        let (_temp, config, paths) = fixture();
        promptforge_gateway_config::write_shadow(
            &paths.profiles_dir.join("main.toml"),
            "not toml [[[",
        )
        .expect("stage a corrupt shadow");
        let addr = serve_with_paths(config, paths).await;

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "a corrupt chain file fails the read loudly"
        );
    }

    #[tokio::test]
    async fn put_env_refuses_an_unknown_scope() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/admin/env?scope=global"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "X": "y" }))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "an unknown scope is refused before any write"
        );
    }

    #[test]
    fn render_value_picks_the_form_dotenv_reads_back_verbatim() {
        use super::render_value;

        // Bare: every character inert.
        assert_eq!(render_value("abc-123"), Some("abc-123".to_owned()));
        assert_eq!(render_value(""), Some(String::new()));
        // Single quotes: whitespace, comments, and `$` stay literal.
        assert_eq!(render_value("two words"), Some("'two words'".to_owned()));
        assert_eq!(render_value("pa$$word"), Some("'pa$$word'".to_owned()));
        // Double quotes: only when a single quote forces them and nothing
        // double quoting gives meaning to is present.
        assert_eq!(render_value("it's"), Some("\"it's\"".to_owned()));
        // Refused: no quoting form can carry these.
        assert_eq!(render_value("a\nb"), None, "an embedded newline");
        assert_eq!(render_value("a\rb"), None, "an embedded carriage return");
        assert_eq!(render_value("a\0b"), None, "an embedded NUL");
        assert_eq!(
            render_value("it's $HOME"),
            None,
            "a single quote mixed with a substitution trigger"
        );
        assert_eq!(
            render_value(r#"it's a "mix" too"#),
            None,
            "a single quote mixed with double quotes"
        );
    }

    #[tokio::test]
    async fn a_malformed_env_body_maps_into_the_error_envelope_after_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();

        let unauthed = http
            .put(format!("http://{addr}/admin/env"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{not json")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            unauthed.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "auth wins over a malformed body"
        );

        let authed = http
            .put(format!("http://{addr}/admin/env"))
            .bearer_auth("test-token")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{not json")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(authed.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = authed.json().await.expect("a JSON envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
    }

    #[tokio::test]
    async fn env_routes_require_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();

        let get = http
            .get(format!("http://{addr}/admin/env"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(get.status(), reqwest::StatusCode::UNAUTHORIZED);

        let put = http
            .put(format!("http://{addr}/admin/env"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(put.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
}
