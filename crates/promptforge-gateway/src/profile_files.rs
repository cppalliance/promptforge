//! Profile file management: `POST /admin/profiles/{name}` and
//! `DELETE /admin/profiles/{name}`.
//!
//! Unlike the shadow-write routes, these manage REAL files in the
//! profiles directory: creating a profile is not a pending edit, so the
//! new file lands directly (atomically, temp + rename) and deletion
//! removes the real file. Three creation modes cover the New Profile
//! dialog: `empty` writes a zero-byte file (a valid, empty TOML table -
//! the profile fails a switch, visibly, until it gains content), `copy`
//! duplicates another profile's real file verbatim (a pending shadow on
//! the source is a staged edit, not the profile, and is ignored), and
//! `include` writes a leaf whose only content is
//! `include = ["<from>.toml"]`.
//!
//! Deletion refuses the active profile (the gateway is running it) and
//! removes the target's `.toml.next` shadow alongside the real file, so
//! a later profile of the same name can never resurrect stale pending
//! edits. Deletion does not check whether other profiles include the
//! target: the next load of an including profile fails visibly with a
//! missing-include error, which is the honest signal.

use std::fs;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use promptforge_gateway_config::{ProfileName, promote_shadow, shadow_path, write_shadow};
use serde::Deserialize;

use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `POST /admin/profiles/{name}` body: which creation mode to use and,
/// for `copy` and `include`, the source profile.
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub(crate) enum CreateProfileBody {
    /// Create `profiles/{name}.toml` as an empty file.
    Empty,
    /// Copy `profiles/{from}.toml`'s real content verbatim, ignoring any
    /// pending shadow on the source.
    Copy {
        /// The profile to copy from.
        from: String,
    },
    /// Create a leaf whose only content is `include = ["{from}.toml"]`.
    Include {
        /// The profile the new leaf includes.
        from: String,
    },
}

/// The `POST /admin/profiles/{name}` route: bearer-authed, creates
/// `profiles/{name}.toml` atomically (temp + rename).
///
/// `{name}` is caller input and crosses the trust boundary through
/// [`parse_name`]. Creation refuses a profile that already exists (409),
/// a `from` profile that does not exist (404), and a `copy` or `include`
/// whose `from` names the new profile itself (400). The write serializes
/// on the apply lock with the shadow saves, apply, and revert.
pub(crate) async fn admin_create_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<CreateProfileBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractors keeps auth first and puts rejections in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let UrlPath(raw) =
        raw.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let Json(body) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let name = parse_name(&raw)?;
    // Creation shares the apply lock so it cannot interleave with a save,
    // apply, or revert walking the profiles directory.
    let _guard = state.apply.lock().await;
    let dir = crate::profiles_dir(&state)?.to_path_buf();
    let created = tokio::task::spawn_blocking(move || create_profile_file(&dir, &name, body))
        .await
        .map_err(|join| GatewayError::ProfileFileIo(Box::new(join)))??;
    Ok(Json(
        serde_json::json!({ "created": created.display().to_string() }),
    ))
}

/// The `DELETE /admin/profiles/{name}` route: bearer-authed, deletes
/// `profiles/{name}.toml` and its `.toml.next` shadow when one exists.
///
/// Refuses the active profile (409; the comparison ignores ASCII case so
/// a case-variant name cannot delete the active profile's file on a
/// case-insensitive filesystem) and a profile that is not on disk (404).
/// The check and the removal run under the switch lock, so a concurrent
/// `POST /admin/switch-profile` cannot make the target active between
/// them. Deleting a profile that other profiles include is allowed: the
/// next load of an including profile fails visibly.
pub(crate) async fn admin_delete_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let UrlPath(raw) =
        raw.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let name = parse_name(&raw)?;
    // Deletion shares the apply lock so it cannot interleave with a save,
    // apply, or revert walking the profiles directory, and holds the
    // switch lock so no switch is mid-flight: without it, a concurrent
    // `POST /admin/switch-profile` could make the target active between
    // the check below and the removal, deleting the running profile's
    // file. Lock order apply -> switch -> live matches the apply route's
    // (apply, then `run_switch` takes switch, then live), so no path
    // acquires these locks in a conflicting order.
    let _guard = state.apply.lock().await;
    let _switch_guard = state.switch.lock().await;
    let active = state.live.read().await.profile_name.clone();
    if let Some(active) = active
        && active.eq_ignore_ascii_case(&name.to_string())
    {
        return Err(GatewayError::ProfileActive(active));
    }
    let dir = crate::profiles_dir(&state)?.to_path_buf();
    let (deleted, shadow_removed) =
        tokio::task::spawn_blocking(move || delete_profile_file(&dir, &name))
            .await
            .map_err(|join| GatewayError::ProfileFileIo(Box::new(join)))??;
    Ok(Json(serde_json::json!({
        "deleted": deleted.display().to_string(),
        "shadow_removed": shadow_removed,
    })))
}

/// File-name stems Windows reserves for devices; a `CON.toml` (or
/// `CON.anything.toml`) is unopenable or aliases a device there, so both
/// routes refuse these on every platform to keep a profile tree portable.
const RESERVED_STEMS: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Parses a profile name at the trust boundary.
///
/// The rule: the name must parse as a [`ProfileName`] (exactly one normal
/// path component - no separators, no `.`/`..` traversal, no NUL, not
/// empty), and its leading dot-delimited stem must not be a Windows
/// reserved device name ([`RESERVED_STEMS`]).
fn parse_name(raw: &str) -> Result<ProfileName, GatewayError> {
    let name = ProfileName::parse(raw).map_err(|error| {
        GatewayError::MalformedRequest(format!("invalid profile name {raw:?}: {error}"))
    })?;
    let text = name.to_string();
    let stem = text.split('.').next().unwrap_or(&text);
    if RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return Err(GatewayError::MalformedRequest(format!(
            "profile name {text:?} is a reserved device name on Windows"
        )));
    }
    Ok(name)
}

/// Creates `dir/<name>.toml` with the mode's content, atomically.
///
/// The write reuses the shadow primitives: the content lands in the
/// target's `.toml.next` via temp + rename, then one more rename promotes
/// it to the real file, so a reader never observes a half-written
/// profile.
fn create_profile_file(
    dir: &Path,
    name: &ProfileName,
    body: CreateProfileBody,
) -> Result<PathBuf, GatewayError> {
    let target = dir.join(format!("{name}.toml"));
    if target.exists() {
        return Err(GatewayError::ProfileExists(name.to_string()));
    }
    let contents = match body {
        CreateProfileBody::Empty => String::new(),
        CreateProfileBody::Copy { from } => {
            let (_, source) = source_profile(dir, name, &from)?;
            fs::read_to_string(&source).map_err(profile_io)?
        }
        CreateProfileBody::Include { from } => {
            let (from, _) = source_profile(dir, name, &from)?;
            include_document(&from)?
        }
    };
    fs::create_dir_all(dir).map_err(profile_io)?;
    write_shadow(&target, &contents).map_err(profile_io)?;
    if let Err(error) = promote_shadow(&target) {
        // A failed promotion must not leave a stray shadow for a profile
        // that was never created; removal is best-effort.
        let _ = fs::remove_file(shadow_path(&target));
        return Err(profile_io(error));
    }
    Ok(target)
}

/// Resolves the `from` profile for `copy` and `include`: same name rule
/// as the target, no self-reference, and the file must exist.
fn source_profile(
    dir: &Path,
    name: &ProfileName,
    from: &str,
) -> Result<(ProfileName, PathBuf), GatewayError> {
    let from = parse_name(from)?;
    if from == *name {
        return Err(GatewayError::MalformedRequest(format!(
            "cannot create profile {name} from itself"
        )));
    }
    let source = dir.join(format!("{from}.toml"));
    if !source.is_file() {
        return Err(GatewayError::ProfileNotFound(from.to_string()));
    }
    Ok((from, source))
}

/// Renders the `include` mode's whole document: one `include` array
/// naming `<from>.toml`, serialized through the TOML writer so the file
/// name is escaped correctly.
fn include_document(from: &ProfileName) -> Result<String, GatewayError> {
    let mut table = toml::map::Map::new();
    table.insert(
        "include".to_owned(),
        toml::Value::Array(vec![toml::Value::String(format!("{from}.toml"))]),
    );
    toml::to_string(&toml::Value::Table(table)).map_err(profile_io)
}

/// Deletes `dir/<name>.toml` and its shadow, reporting whether a shadow
/// was removed.
fn delete_profile_file(dir: &Path, name: &ProfileName) -> Result<(PathBuf, bool), GatewayError> {
    let target = dir.join(format!("{name}.toml"));
    if !target.is_file() {
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }
    fs::remove_file(&target).map_err(profile_io)?;
    let shadow = shadow_path(&target);
    let shadow_removed = shadow.is_file();
    if shadow_removed {
        fs::remove_file(&shadow).map_err(profile_io)?;
    }
    Ok((target, shadow_removed))
}

/// Wraps a filesystem or serialization failure as the 500-class
/// [`GatewayError::ProfileFileIo`].
fn profile_io(source: impl std::error::Error + Send + Sync + 'static) -> GatewayError {
    GatewayError::ProfileFileIo(Box::new(source))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use promptforge_gateway_config::{Config, ProfileName, shadow_path, write_shadow};

    use crate::test_support::{AdminPaths, serve_with_paths};

    /// The boot catalog: server key, one endpoint, one model.
    const BOOT: &str = r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "s"

[[model]]
name = "m"
description = "prose"
context = 1024
upstream = "u"
endpoints = ["e"]
"#;

    /// An included parent profile, the `copy`/`include` source.
    const COMMON: &str = r#"
[[endpoint]]
id = "e2"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "common-secret"
"#;

    const MAIN: &str = "include = [\"../gateway.toml\", \"common.toml\"]\n";

    /// A tempdir with `gateway.toml`, `profiles/common.toml`, and an
    /// active `main` profile including both.
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

    async fn create(
        addr: std::net::SocketAddr,
        name: &str,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/admin/profiles/{name}"))
            .bearer_auth("test-token")
            .json(body)
            .send()
            .await
            .expect("the request sends")
    }

    async fn delete(addr: std::net::SocketAddr, name: &str) -> reqwest::Response {
        reqwest::Client::new()
            .delete(format!("http://{addr}/admin/profiles/{name}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends")
    }

    /// Sorted file names in the profiles directory.
    fn dir_listing(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read profiles dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort_unstable();
        names
    }

    #[tokio::test]
    async fn create_writes_empty_copy_and_include_files() {
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;

        let response = create(addr, "blank", &serde_json::json!({ "mode": "empty" })).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(dir.join("blank.toml")).expect("read blank"),
            "",
            "empty mode writes a zero-byte file"
        );

        let response = create(
            addr,
            "copied",
            &serde_json::json!({ "mode": "copy", "from": "common" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(dir.join("copied.toml")).expect("read copied"),
            COMMON,
            "copy mode duplicates the source verbatim"
        );

        let response = create(
            addr,
            "leaf",
            &serde_json::json!({ "mode": "include", "from": "common" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(dir.join("leaf.toml")).expect("read leaf"),
            "include = [\"common.toml\"]\n",
            "include mode's only content is the include line"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("common.toml")).expect("re-read common"),
            COMMON,
            "the source file is untouched"
        );
    }

    #[tokio::test]
    async fn copy_reads_the_real_file_never_the_pending_shadow() {
        // The copy contract is verbatim-real: a pending shadow on the
        // source is a staged edit, not the profile, so it must not leak
        // into the new file.
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;
        let source = dir.join("common.toml");
        write_shadow(&source, "pending = 1\n").expect("stage a source shadow");

        let response = create(
            addr,
            "copied",
            &serde_json::json!({ "mode": "copy", "from": "common" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(dir.join("copied.toml")).expect("read copied"),
            COMMON,
            "the copy carries the real file's bytes, not the shadow's"
        );
        assert_eq!(
            std::fs::read_to_string(shadow_path(&source)).expect("read source shadow"),
            "pending = 1\n",
            "the source's pending shadow is untouched"
        );
    }

    #[tokio::test]
    async fn create_refuses_existing_invalid_missing_from_and_self() {
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;
        let empty = serde_json::json!({ "mode": "empty" });

        let response = create(addr, "main", &empty).await;
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        let body: serde_json::Value = response.json().await.expect("a JSON envelope");
        assert_eq!(body["error"]["code"], "profile_exists");

        // Encoded so the URL parser cannot collapse the segments before
        // the handler sees them (a bare `%2e%2e` segment is a WHATWG
        // double-dot segment and never reaches the route; the trailing
        // space survives the parser and trims away in the handler).
        for encoded in [
            "%2e%2e%20",       // ".. ", trimmed to ..
            "%2e%2e%2fescape", // ../escape
            "a%2Fb",           // a/b
            "a%5Cb",           // a\b
            "%20",             // a name that trims to empty
            "CON",             // Windows reserved device name
            "nul.backup",      // reserved stem before the first dot
        ] {
            let response = create(addr, encoded, &empty).await;
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "{encoded} must be refused at the trust boundary"
            );
        }

        let response = create(
            addr,
            "fresh",
            &serde_json::json!({ "mode": "copy", "from": "ghost" }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "a missing `from` profile is refused"
        );

        let response = create(
            addr,
            "fresh",
            &serde_json::json!({ "mode": "copy", "from": "../gateway" }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "`from` crosses the same trust boundary as the name"
        );

        let response = create(
            addr,
            "fresh",
            &serde_json::json!({ "mode": "include", "from": "fresh" }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "a self-referencing `from` is refused"
        );

        let response = create(addr, "fresh", &serde_json::json!({ "mode": "copy" })).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("a JSON envelope");
        assert_eq!(
            body["error"]["code"], "malformed_request",
            "a copy body without `from` lands in the gateway envelope"
        );

        assert_eq!(
            dir_listing(&dir),
            ["common.toml", "main.toml"],
            "no refused request may create a file"
        );
    }

    #[tokio::test]
    async fn delete_removes_the_file_and_its_shadow() {
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;
        let stray = dir.join("stray.toml");
        std::fs::write(&stray, "").expect("write stray");
        write_shadow(&stray, "pending = 1\n").expect("write stray shadow");

        let response = delete(addr, "stray").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(body["shadow_removed"], true);
        assert!(!stray.exists(), "the real file is deleted");
        assert!(
            !shadow_path(&stray).exists(),
            "the shadow is deleted alongside the real file"
        );
    }

    #[tokio::test]
    async fn delete_refuses_the_active_profile_and_a_missing_one() {
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;

        let response = delete(addr, "main").await;
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        let body: serde_json::Value = response.json().await.expect("a JSON envelope");
        assert_eq!(body["error"]["code"], "profile_active");
        assert!(
            dir.join("main.toml").is_file(),
            "the active profile's file survives the refused delete"
        );

        let response = delete(addr, "MAIN").await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::CONFLICT,
            "a case-variant of the active name is refused, so a \
             case-insensitive filesystem cannot lose the active file"
        );

        let response = delete(addr, "ghost").await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn profile_file_routes_require_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let dir = paths.profiles_dir.clone();
        let addr = serve_with_paths(config, paths).await;
        let http = reqwest::Client::new();

        let response = http
            .post(format!("http://{addr}/admin/profiles/unauthed"))
            .json(&serde_json::json!({ "mode": "empty" }))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "create must refuse a request without the bearer key"
        );

        let response = http
            .delete(format!("http://{addr}/admin/profiles/common"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "delete must refuse a request without the bearer key"
        );

        assert_eq!(
            dir_listing(&dir),
            ["common.toml", "main.toml"],
            "no unauthenticated request may touch the profiles directory"
        );
    }

    #[test]
    fn parse_name_states_the_rule() {
        for good in ["dev", "analysis..v2", ".hidden", "concert"] {
            assert!(
                super::parse_name(good).is_ok(),
                "{good:?} is a single normal component and not reserved"
            );
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "COM7", "Nul.old"] {
            assert!(super::parse_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn include_document_escapes_the_file_name() {
        // A name TOML cannot carry in a plain quoted string must still
        // produce a parseable document, not a broken literal.
        let from = ProfileName::parse("with\"quote").expect("a single component");
        let document = super::include_document(&from).expect("renders");
        let parsed: toml::Value = toml::from_str(&document).expect("the document parses");
        assert_eq!(
            parsed["include"][0].as_str(),
            Some("with\"quote.toml"),
            "the include entry round-trips the exact file name"
        );
    }

    #[test]
    fn create_profile_file_leaves_no_shadow_behind() {
        // The atomic write goes through the shadow path; a successful
        // create must leave only the real file.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let name = ProfileName::parse("fresh").expect("name");
        let created =
            super::create_profile_file(temp.path(), &name, super::CreateProfileBody::Empty)
                .expect("creates");
        assert_eq!(created, temp.path().join("fresh.toml"));
        assert!(created.is_file());
        assert!(
            !shadow_path(&created).exists(),
            "the temp shadow is promoted away, not left behind"
        );
        let listing: Vec<PathBuf> = std::fs::read_dir(temp.path())
            .expect("read dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        assert_eq!(listing, [created], "no other litter remains");
    }
}
