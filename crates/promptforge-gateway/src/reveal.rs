//! The `POST /admin/reveal` route: opens the host OS file manager at a
//! cache or profile path, for the UI's "reveal in folder" button on model
//! files and config files.
//!
//! The endpoint launches a process, so it is guarded three ways. The
//! caller must be on the loopback interface: `build_router` places the
//! route behind the shared loopback wall from
//! `promptforge-gateway-loopback`, which refuses any non-loopback or
//! unknown peer with a bare 403 before this handler ever runs (and
//! before auth). The caller must present the bearer key (401). And the named
//! path must canonicalize to strictly inside one of the known-safe roots -
//! the artifact cache and the profiles directory (a root itself is
//! refused, so the non-Windows parent-directory reveal can never name a
//! directory outside every root; 400 otherwise, 404 when the path does
//! not exist). The path never crosses a shell: the file manager is
//! spawned directly with separate arguments, through an injectable
//! [`RevealLauncher`] so tests assert the exact constructed command
//! without spawning anything.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;

use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The `POST /admin/reveal` body: the filesystem path to reveal.
#[derive(Debug, Deserialize)]
pub(crate) struct RevealRequest {
    /// Path of the file or directory to reveal. Must exist and must
    /// canonicalize to strictly inside the artifact cache or the profiles
    /// directory; the roots themselves are refused.
    pub(crate) path: String,
}

/// The command a reveal resolves to: the file-manager program and its
/// arguments, each a separate `OsString` so no shell ever interprets the
/// path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevealCommand {
    /// The program to spawn (the absolute `explorer.exe`, or `open`, or
    /// `xdg-open`).
    pub(crate) program: OsString,
    /// The program's arguments, passed separately, never joined.
    pub(crate) args: Vec<OsString>,
}

/// Launches a [`RevealCommand`]; injectable so tests observe the
/// constructed command without spawning a process.
pub(crate) trait RevealLauncher: Send + Sync + std::fmt::Debug {
    /// Launches `command` without waiting for it to exit.
    ///
    /// # Errors
    /// Returns the spawn failure when the program cannot start.
    fn launch(&self, command: RevealCommand) -> std::io::Result<()>;
}

/// The production launcher: spawns the command and does not wait.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpawnLauncher;

impl RevealLauncher for SpawnLauncher {
    fn launch(&self, command: RevealCommand) -> std::io::Result<()> {
        // Fire and forget: the file manager outlives the request and its
        // exit status means nothing to the caller, so the child handle is
        // dropped as soon as the spawn succeeds.
        std::process::Command::new(&command.program)
            .args(&command.args)
            .spawn()
            .map(drop)
    }
}

/// The `POST /admin/reveal` route: loopback-only and bearer-authed, opens
/// the OS file manager at the request's path and replies 204 without
/// waiting for the spawned process. The loopback wall is not this
/// handler's: `build_router` layers the shared `require_loopback`
/// middleware over the route, so a non-loopback or unknown peer is
/// refused with a bare 403 before auth and before this body runs.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] on a
/// missing or wrong bearer key, [`GatewayError::MalformedRequest`] when
/// the body does not parse or the path resolves outside every safe root
/// (or is a root itself),
/// [`GatewayError::RevealPathNotFound`] when the path does not exist, and
/// [`GatewayError::RevealFailed`] when the file manager cannot spawn.
pub(crate) async fn admin_reveal(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<RevealRequest>, JsonRejection>,
) -> Result<StatusCode, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps the guards first and puts the rejection
    // in the gateway's JSON error envelope instead of axum's plain-text 400.
    let Json(request) =
        body.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;

    let mut roots: Vec<PathBuf> = Vec::new();
    #[cfg(feature = "local")]
    {
        let config = {
            let live = state.live.read().await;
            Arc::clone(&live.config)
        };
        // An unresolvable cache root (no cache_dir configured and no home
        // directory) contributes no safe root; a reveal aimed at the
        // profiles directory must still work.
        if let Ok(root) = crate::local::resolve_cache_root(config.local().cache_dir()) {
            roots.push(root);
        }
    }
    // A gateway assembled without a profiles directory offers no config
    // root; the cache root (when present) still confines reveals.
    if let Ok(dir) = crate::profiles_dir(&state) {
        roots.push(dir.to_path_buf());
    }

    // Canonicalization and the spawn are blocking filesystem work.
    let launcher = Arc::clone(&state.reveal);
    tokio::task::spawn_blocking(move || {
        let command = resolve_reveal(&roots, Path::new(&request.path))?;
        launcher
            .launch(command)
            .map_err(|error| GatewayError::RevealFailed(Box::new(error)))
    })
    .await
    .map_err(|join| GatewayError::RevealFailed(Box::new(join)))??;
    Ok(StatusCode::NO_CONTENT)
}

/// Confines `path` to strictly inside the safe roots and builds the
/// platform's reveal command for it.
///
/// Both sides of the containment check are canonicalized, so `..`
/// segments, relative forms, and symlinks are resolved before comparison.
///
/// # Errors
/// Returns [`GatewayError::RevealPathNotFound`] when `path` does not
/// exist, [`GatewayError::RevealFailed`] when canonicalization fails for
/// another reason, and [`GatewayError::MalformedRequest`] when the
/// canonical path lies outside every root or is a root itself.
fn resolve_reveal(roots: &[PathBuf], path: &Path) -> Result<RevealCommand, GatewayError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            GatewayError::RevealPathNotFound(path.display().to_string())
        } else {
            GatewayError::RevealFailed(Box::new(error))
        }
    })?;
    // Each root canonicalizes independently so the prefix comparison
    // happens in one namespace; a root that cannot canonicalize (a cache
    // dir never created, say) confines nothing rather than failing a
    // reveal aimed at another root. Containment is strict: a root itself
    // is refused, so the non-Windows parent-directory reveal can never
    // hand the launcher a directory outside every root.
    let confined = roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| canonical.starts_with(&root) && canonical != root);
    if !confined {
        return Err(GatewayError::MalformedRequest(format!(
            "path `{}` is not inside the artifact cache or the profiles directory",
            path.display()
        )));
    }
    Ok(reveal_command(&canonical))
}

/// The Windows reveal: `explorer.exe /select,<path>` highlights the target
/// in its parent folder. The two tokens stay separate arguments; explorer
/// accepts the split form, and nothing is ever joined through a shell.
/// The program is the absolute `%WINDIR%\explorer.exe`, because
/// `CreateProcess` resolves an unqualified name through the current
/// directory before the system directories, and a planted `explorer.exe`
/// must not win that search.
#[cfg(windows)]
fn reveal_command(target: &Path) -> RevealCommand {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    RevealCommand {
        program: Path::new(&windir).join("explorer.exe").into_os_string(),
        args: vec![OsString::from("/select,"), strip_verbatim(target)],
    }
}

/// The non-Windows reveal: neither `open` nor `xdg-open` can select a
/// file, so the closest equivalent is opening the target's parent
/// directory. Confinement is strict (a root itself is never revealed),
/// so the parent stays inside a safe root; the fallback to the target
/// itself only keeps the function total for a parentless path.
#[cfg(not(windows))]
fn reveal_command(target: &Path) -> RevealCommand {
    let directory = target.parent().unwrap_or(target);
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    RevealCommand {
        program: OsString::from(program),
        args: vec![directory.as_os_str().to_owned()],
    }
}

/// Rewrites a verbatim path to its plain form for explorer.exe.
///
/// `fs::canonicalize` returns verbatim (`\\?\`) paths on Windows and
/// explorer.exe does not accept that prefix
/// (<https://github.com/rust-lang/rust/issues/42869>), so the command
/// carries `C:\...` or `\\server\share\...` instead.
#[cfg(windows)]
fn strip_verbatim(path: &Path) -> OsString {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return OsString::from(format!(r"\\{unc}"));
    }
    if let Some(disk) = text.strip_prefix(r"\\?\") {
        return OsString::from(disk.to_owned());
    }
    path.as_os_str().to_owned()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{Request, StatusCode};
    use promptforge_gateway_config::Config;
    use tower::ServiceExt;

    use super::{RevealCommand, RevealLauncher};
    use crate::test_support::{AdminPaths, app_state, serve_state};

    /// A launcher that records every command instead of spawning.
    #[derive(Debug, Default)]
    struct RecordingLauncher {
        commands: Mutex<Vec<RevealCommand>>,
    }

    impl RecordingLauncher {
        fn commands(&self) -> Vec<RevealCommand> {
            self.commands
                .lock()
                .expect("the recording mutex is never poisoned")
                .clone()
        }
    }

    impl RevealLauncher for RecordingLauncher {
        fn launch(&self, command: RevealCommand) -> std::io::Result<()> {
            self.commands
                .lock()
                .expect("the recording mutex is never poisoned")
                .push(command);
            Ok(())
        }
    }

    /// A tempdir with a cache root holding `models/tiny.gguf`, a profiles
    /// directory holding `main.toml`, and an `outside.txt` in neither.
    fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("cache").join("models");
        std::fs::create_dir_all(&models).expect("mkdir cache models");
        std::fs::write(models.join("tiny.gguf"), b"stub").expect("write model");
        let profiles = temp.path().join("profiles");
        std::fs::create_dir(&profiles).expect("mkdir profiles");
        std::fs::write(profiles.join("main.toml"), "").expect("write profile");
        std::fs::write(temp.path().join("outside.txt"), "outside").expect("write outsider");
        let boot = temp.path().join("gateway.toml");
        std::fs::write(&boot, "").expect("write boot");
        let config = Config::from_toml_str(&format!(
            r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache}'
"#,
            cache = temp.path().join("cache").display(),
        ))
        .expect("the fixture profile parses");
        let paths = AdminPaths {
            profiles_dir: profiles,
            active: "main".to_owned(),
            boot_config: boot,
        };
        (temp, config, paths)
    }

    /// Serves the fixture with a recording launcher injected.
    async fn serve_reveal(
        config: Config,
        paths: AdminPaths,
    ) -> (SocketAddr, Arc<RecordingLauncher>) {
        let launcher = Arc::new(RecordingLauncher::default());
        let mut state = app_state(config, Some(paths));
        state.reveal = Arc::clone(&launcher) as Arc<dyn RevealLauncher>;
        (serve_state(state).await, launcher)
    }

    /// POSTs `/admin/reveal` for `path`, with `token` as the bearer when
    /// given.
    async fn post_reveal(addr: SocketAddr, token: Option<&str>, path: &str) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .post(format!("http://{addr}/admin/reveal"))
            .json(&serde_json::json!({ "path": path }));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.expect("the request sends")
    }

    /// The command the platform must construct for `target`, computed
    /// independently of the module's own helpers.
    fn expected_command(target: &Path) -> RevealCommand {
        let canonical = std::fs::canonicalize(target).expect("the target canonicalizes");
        #[cfg(windows)]
        {
            let plain = canonical
                .to_string_lossy()
                .strip_prefix(r"\\?\")
                .expect("canonicalize returns a verbatim path on Windows")
                .to_owned();
            let windir = std::env::var_os("WINDIR").expect("Windows sets WINDIR");
            RevealCommand {
                program: Path::new(&windir).join("explorer.exe").into_os_string(),
                args: vec!["/select,".into(), plain.into()],
            }
        }
        #[cfg(not(windows))]
        {
            let parent = canonical
                .parent()
                .expect("a fixture file has a parent")
                .as_os_str()
                .to_owned();
            let program = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            RevealCommand {
                program: program.into(),
                args: vec![parent],
            }
        }
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn reveal_selects_a_cache_file_in_the_file_manager() {
        let (temp, config, paths) = fixture();
        let target = temp.path().join("cache").join("models").join("tiny.gguf");
        let (addr, launcher) = serve_reveal(config, paths).await;

        let response = post_reveal(addr, Some("test-token"), &target.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert_eq!(
            launcher.commands(),
            vec![expected_command(&target)],
            "the launcher receives exactly the platform's reveal command \
             for the canonical path"
        );
    }

    #[tokio::test]
    async fn reveal_selects_a_profile_file_in_the_file_manager() {
        let (_temp, config, paths) = fixture();
        let target = paths.profiles_dir.join("main.toml");
        let (addr, launcher) = serve_reveal(config, paths).await;

        let response = post_reveal(addr, Some("test-token"), &target.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert_eq!(launcher.commands(), vec![expected_command(&target)]);
    }

    #[tokio::test]
    async fn reveal_rejects_a_path_outside_the_safe_roots() {
        let (temp, config, paths) = fixture();
        // Both exist on disk; neither is under the cache or profiles root
        // (the boot config's own directory is deliberately not a safe root).
        let outsiders = [
            temp.path().join("outside.txt"),
            temp.path().join("gateway.toml"),
        ];
        let (addr, launcher) = serve_reveal(config, paths).await;

        for outsider in outsiders {
            let response =
                post_reveal(addr, Some("test-token"), &outsider.display().to_string()).await;
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "`{}` must be refused at the boundary",
                outsider.display()
            );
            let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
            assert_eq!(body["error"]["code"], "malformed_request");
        }
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for a path outside the safe roots"
        );
    }

    #[tokio::test]
    async fn reveal_refuses_a_traversal_that_resolves_outside() {
        let (temp, config, paths) = fixture();
        // A raw component-prefix check would admit this path (it begins
        // with the cache root's components); only canonicalizing before
        // the comparison resolves the `..` segments to `outside.txt` and
        // refuses it.
        let traversal = temp
            .path()
            .join("cache")
            .join("models")
            .join("..")
            .join("..")
            .join("outside.txt");
        let (addr, launcher) = serve_reveal(config, paths).await;

        let response =
            post_reveal(addr, Some("test-token"), &traversal.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for a traversal that resolves outside"
        );
    }

    #[tokio::test]
    async fn reveal_refuses_the_safe_root_itself() {
        let (_temp, config, paths) = fixture();
        let root = paths.profiles_dir.clone();
        let (addr, launcher) = serve_reveal(config, paths).await;

        // Containment is strict: on non-Windows the reveal opens the
        // target's parent, so revealing a root would hand the launcher a
        // directory outside every root.
        let response = post_reveal(addr, Some("test-token"), &root.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for a safe root itself"
        );
    }

    #[tokio::test]
    async fn a_missing_peer_address_fails_closed_as_non_loopback() {
        let (_temp, config, paths) = fixture();
        let target = paths.profiles_dir.join("main.toml");
        let launcher = Arc::new(RecordingLauncher::default());
        let mut state = app_state(config, Some(paths));
        state.reveal = Arc::clone(&launcher) as Arc<dyn RevealLauncher>;

        // Served WITHOUT connect info, as a misassembled embedding host
        // would: the peer-address extension is absent, and the shared wall
        // must fail closed with its bare 403 rather than admit the caller.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the test listener binds");
        let addr = listener.local_addr().expect("the bound address");
        tokio::spawn(async move {
            let _ignored = axum::serve(listener, crate::build_router(state)).await;
        });

        let response = post_reveal(addr, Some("test-token"), &target.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run when the peer address is unknown"
        );
    }

    #[tokio::test]
    async fn reveal_rejects_a_missing_path() {
        let (_temp, config, paths) = fixture();
        let ghost = paths.profiles_dir.join("ghost.toml");
        let (addr, launcher) = serve_reveal(config, paths).await;

        let response = post_reveal(addr, Some("test-token"), &ghost.display().to_string()).await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "reveal_path_not_found");
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for a missing path"
        );
    }

    #[tokio::test]
    async fn reveal_requires_bearer_auth() {
        let (_temp, config, paths) = fixture();
        let target = paths.profiles_dir.join("main.toml");
        let (addr, launcher) = serve_reveal(config, paths).await;

        for token in [None, Some("wrong-token")] {
            let response = post_reveal(addr, token, &target.display().to_string()).await;
            assert_eq!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "a request with bearer {token:?} is refused"
            );
        }
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for an unauthenticated caller"
        );
    }

    #[tokio::test]
    async fn reveal_refuses_a_non_loopback_caller() {
        let (_temp, config, paths) = fixture();
        let target = paths.profiles_dir.join("main.toml");
        let launcher = Arc::new(RecordingLauncher::default());
        let mut state = app_state(config, Some(paths));
        state.reveal = Arc::clone(&launcher) as Arc<dyn RevealLauncher>;

        // A LAN peer presenting the valid bearer key: the shared loopback
        // wall layered in `build_router` must refuse before auth even
        // matters. A real TCP connection to the test listener is always
        // loopback, so the router is driven in-process with a forged peer
        // address planted in the ConnectInfo extension.
        let body = serde_json::json!({ "path": target.display().to_string() }).to_string();
        let mut request = Request::builder()
            .method("POST")
            .uri("/admin/reveal")
            .header(AUTHORIZATION, "Bearer test-token")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("static request parts are valid");
        let peer: SocketAddr = "198.51.100.7:44821".parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));

        let response = crate::build_router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            launcher.commands().is_empty(),
            "the launcher must never run for a non-loopback caller"
        );
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_unwraps_disk_and_unc_prefixes() {
        use std::ffi::OsString;

        assert_eq!(
            super::strip_verbatim(Path::new(r"\\?\C:\cache\m.gguf")),
            OsString::from(r"C:\cache\m.gguf")
        );
        assert_eq!(
            super::strip_verbatim(Path::new(r"\\?\UNC\host\share\m.gguf")),
            OsString::from(r"\\host\share\m.gguf")
        );
        assert_eq!(
            super::strip_verbatim(Path::new(r"C:\plain")),
            OsString::from(r"C:\plain")
        );
    }
}
