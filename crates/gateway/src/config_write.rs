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

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use gateway_config::{ConfigErrorKind, save_config_shadow};
use rand::Rng as _;

use crate::auth::Caller;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

const PREPARED_CREATE_ATTEMPTS: u64 = 16;
static PERSISTENCE_NAMES: LazyLock<ProcessPreparationNames<fn() -> u128>> =
    LazyLock::new(|| ProcessPreparationNames::new(std::process::id(), random_persistence_nonce));

struct ProcessPreparationNames<N> {
    pid: u32,
    nonce: OnceLock<u128>,
    sequence: AtomicU64,
    random_nonce: N,
}

impl<N: Fn() -> u128> ProcessPreparationNames<N> {
    fn new(pid: u32, random_nonce: N) -> Self {
        Self {
            pid,
            nonce: OnceLock::new(),
            sequence: AtomicU64::new(0),
            random_nonce,
        }
    }

    fn nonce(&self) -> u128 {
        *self.nonce.get_or_init(|| (self.random_nonce)())
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }
}

/// One fully written and synced temporary file awaiting atomic replacement.
#[derive(Debug)]
pub(crate) struct PreparedFile {
    target: PathBuf,
    temporary: Option<PathBuf>,
    original: Option<Vec<u8>>,
    contents: Vec<u8>,
}

impl PreparedFile {
    pub(crate) fn prepare(target: PathBuf, contents: String) -> Result<Self, GatewayError> {
        Self::prepare_with_name_source(target, contents, &PERSISTENCE_NAMES)
    }

    fn prepare_with_name_source<N: Fn() -> u128>(
        target: PathBuf,
        contents: String,
        names: &ProcessPreparationNames<N>,
    ) -> Result<Self, GatewayError> {
        Self::prepare_with_names(target, contents, names.pid, names.nonce(), || {
            names.next_sequence()
        })
    }

    fn prepare_with_names(
        target: PathBuf,
        contents: String,
        pid: u32,
        nonce: u128,
        mut next_sequence: impl FnMut() -> u64,
    ) -> Result<Self, GatewayError> {
        let original = match std::fs::read(&target) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(GatewayError::ConfigWriteIo(Box::new(error))),
        };
        let (mut file, temporary) = create_prepared(&target, pid, nonce, &mut next_sequence)
            .map_err(|error| GatewayError::ConfigWriteIo(Box::new(error)))?;
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(GatewayError::ConfigWriteIo(Box::new(error)));
        }
        Ok(Self {
            target,
            temporary: Some(temporary),
            original,
            contents: contents.into_bytes(),
        })
    }

    pub(crate) fn commit(&mut self) -> Result<(), std::io::Error> {
        let temporary = self.temporary.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "prepared persistence was already committed",
            )
        })?;
        std::fs::rename(temporary, &self.target)?;
        self.temporary = None;
        Ok(())
    }

    pub(crate) fn still_original(&self) -> bool {
        match (&self.original, std::fs::read(&self.target)) {
            (Some(original), Ok(current)) => &current == original,
            (None, Err(error)) => error.kind() == std::io::ErrorKind::NotFound,
            _ => false,
        }
    }

    pub(crate) fn has_committed_contents(&self) -> bool {
        std::fs::read(&self.target).is_ok_and(|current| current == self.contents)
    }

    pub(crate) fn target(&self) -> &Path {
        &self.target
    }

    #[cfg(test)]
    pub(crate) fn discard_temporary(&self) {
        let temporary = self
            .temporary
            .as_ref()
            .expect("uncommitted preparation owns a temporary");
        std::fs::remove_file(temporary).expect("prepared temporary exists");
    }
}

impl Drop for PreparedFile {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = std::fs::remove_file(temporary);
        }
    }
}

fn random_persistence_nonce() -> u128 {
    rand::rng().random()
}

#[derive(Debug)]
struct PreparedCreateExhausted {
    target: PathBuf,
    attempts: u64,
    last_candidate: PathBuf,
    source: std::io::Error,
}

impl std::fmt::Display for PreparedCreateExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to prepare {} after {} create_new attempts; last candidate {}",
            self.target.display(),
            self.attempts,
            self.last_candidate.display()
        )
    }
}

impl std::error::Error for PreparedCreateExhausted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn create_prepared(
    target: &Path,
    pid: u32,
    nonce: u128,
    next_sequence: &mut impl FnMut() -> u64,
) -> Result<(std::fs::File, PathBuf), std::io::Error> {
    let mut last_collision = None;
    for _ in 0..PREPARED_CREATE_ATTEMPTS {
        let temporary = persistence_temporary(target, pid, nonce, next_sequence());
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some((temporary, error));
            }
            Err(error) => return Err(error),
        }
    }
    let Some((last_candidate, source)) = last_collision else {
        return Err(std::io::Error::other(
            "prepared persistence retry budget must be nonzero",
        ));
    };
    Err(std::io::Error::new(
        source.kind(),
        PreparedCreateExhausted {
            target: target.to_path_buf(),
            attempts: PREPARED_CREATE_ATTEMPTS,
            last_candidate,
            source,
        },
    ))
}

fn persistence_temporary(target: &Path, pid: u32, nonce: u128, sequence: u64) -> PathBuf {
    let mut name = target
        .file_name()
        .map_or_else(|| "profile".into(), std::ffi::OsStr::to_os_string);
    name.push(format!(".prepared-{pid}-{nonce:032x}-{sequence}"));
    target.with_file_name(name)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the cross-platform contract reports Unix directory sync failures; unsupported platforms are a no-op"
)]
pub(crate) fn sync_parent(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        std::fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

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

    #[test]
    fn prepared_file_retries_deterministic_collisions_without_claiming_residue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        std::fs::write(&target, "old").expect("write target");
        let pid = 41;
        let nonce = 0x1234;
        let collision = super::persistence_temporary(&target, pid, nonce, 7);
        let owned = super::persistence_temporary(&target, pid, nonce, 8);
        std::fs::write(&collision, "crash residue").expect("write collision");
        let mut sequences = [7, 8].into_iter();

        let prepared =
            super::PreparedFile::prepare_with_names(target, "new".to_owned(), pid, nonce, || {
                sequences.next().expect("bounded sequence")
            })
            .expect("collision retries");

        assert_eq!(
            std::fs::read_to_string(&collision).expect("read residue"),
            "crash residue"
        );
        assert_eq!(
            std::fs::read_to_string(&owned).expect("read preparation"),
            "new"
        );
        drop(prepared);
        assert!(collision.exists(), "unowned residue remains");
        assert!(!owned.exists(), "owned preparation is cleaned");
    }

    #[test]
    fn process_name_source_is_stable_full_width_and_unique_across_pid_reuse() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 73;
        let first_nonce = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;
        let second_nonce = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdef;
        let first_nonce_calls = std::cell::Cell::new(0);
        let first_source = super::ProcessPreparationNames::new(pid, || {
            first_nonce_calls.set(first_nonce_calls.get() + 1);
            first_nonce
        });
        let second_source = super::ProcessPreparationNames::new(pid, || second_nonce);

        let first = super::PreparedFile::prepare_with_name_source(
            target.clone(),
            "first preparation".to_owned(),
            &first_source,
        )
        .expect("first process prepares");
        let next = super::PreparedFile::prepare_with_name_source(
            target.clone(),
            "next preparation".to_owned(),
            &first_source,
        )
        .expect("same process prepares again");
        let reused = super::PreparedFile::prepare_with_name_source(
            target,
            "reused PID preparation".to_owned(),
            &second_source,
        )
        .expect("reused PID prepares");

        assert_eq!(first_nonce_calls.get(), 1, "one nonce per process source");
        assert_eq!(
            first
                .temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-0123456789abcdeffedcba9876543210-0"
            ))
        );
        assert_eq!(
            next.temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-0123456789abcdeffedcba9876543210-1"
            ))
        );
        assert_eq!(
            reused
                .temporary
                .as_deref()
                .and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new(
                "gateway.state.toml.prepared-73-fedcba98765432100123456789abcdef-0"
            ))
        );
    }

    #[test]
    fn process_nonce_separates_pid_reuse_from_crash_residue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 73;
        let crashed = super::persistence_temporary(&target, pid, 0xaaaa, 0);
        let current = super::persistence_temporary(&target, pid, 0xbbbb, 0);
        std::fs::write(&crashed, "prior process").expect("write crash residue");

        let prepared = super::PreparedFile::prepare_with_names(
            target,
            "current process".to_owned(),
            pid,
            0xbbbb,
            || 0,
        )
        .expect("reused PID prepares");

        assert_eq!(
            std::fs::read_to_string(&crashed).expect("read crash residue"),
            "prior process"
        );
        assert_eq!(
            std::fs::read_to_string(&current).expect("read current preparation"),
            "current process"
        );
        drop(prepared);
        assert!(crashed.exists(), "prior process residue remains");
    }

    #[test]
    fn prepared_file_bounds_collision_retries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        let pid = 97;
        let nonce = 0xcafe;
        for sequence in 0..super::PREPARED_CREATE_ATTEMPTS {
            std::fs::write(
                super::persistence_temporary(&target, pid, nonce, sequence),
                format!("residue {sequence}"),
            )
            .expect("write residue");
        }
        let mut sequence = 0_u64;

        let error = super::PreparedFile::prepare_with_names(
            target.clone(),
            "new".to_owned(),
            pid,
            nonce,
            || {
                let current = sequence;
                sequence += 1;
                current
            },
        )
        .expect_err("retry budget exhausts");

        let super::GatewayError::ConfigWriteIo(error) = error else {
            panic!("collision exhaustion returns an I/O error");
        };
        let error = error.downcast_ref::<std::io::Error>().expect("I/O source");
        let last_candidate =
            super::persistence_temporary(&target, pid, nonce, super::PREPARED_CREATE_ATTEMPTS - 1);
        assert_eq!(
            error.to_string(),
            format!(
                "failed to prepare {} after {} create_new attempts; last candidate {}",
                target.display(),
                super::PREPARED_CREATE_ATTEMPTS,
                last_candidate.display()
            )
        );
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let context = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<super::PreparedCreateExhausted>())
            .expect("collision exhaustion context");
        assert_eq!(context.attempts, super::PREPARED_CREATE_ATTEMPTS);
        assert_eq!(context.last_candidate, last_candidate);
        let collision = std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .expect("final collision source");
        assert_eq!(collision.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(sequence, super::PREPARED_CREATE_ATTEMPTS);
        for residue in 0..super::PREPARED_CREATE_ATTEMPTS {
            assert_eq!(
                std::fs::read_to_string(
                    super::persistence_temporary(&target, pid, nonce, residue,)
                )
                .expect("read residue"),
                format!("residue {residue}")
            );
        }
    }

    #[test]
    fn successful_commit_releases_temporary_path_ownership() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("gateway.state.toml");
        std::fs::write(&target, "old").expect("write target");
        let temporary = super::persistence_temporary(&target, 101, 0xfeed, 3);
        let mut prepared = super::PreparedFile::prepare_with_names(
            target.clone(),
            "new".to_owned(),
            101,
            0xfeed,
            || 3,
        )
        .expect("prepare");

        prepared.commit().expect("commit");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "new"
        );
        assert!(!temporary.exists(), "rename consumes preparation");
        std::fs::write(&temporary, "later owner").expect("replace temporary path");
        drop(prepared);
        assert_eq!(
            std::fs::read_to_string(&temporary).expect("read later owner"),
            "later owner"
        );
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
