//! The `POST /admin/reveal` route: opens the host OS file manager at a
//! cache or profile path, for the UI's "reveal in folder" button on model
//! files and config files.
//!
//! The endpoint launches a process, so it is guarded three ways. The
//! caller must be on the loopback interface: `build_router` places the
//! route behind the shared loopback wall from
//! `shared-loopback`, which refuses any non-loopback or
//! unknown peer with a bare 403 before this handler ever runs (and
//! before auth). The caller must present the bearer key (401). And the named
//! path must canonicalize to strictly inside the artifact cache (the root is
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

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::routing::post;
use serde::Deserialize;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, WireJson, blocking};
use crate::registry::RouteInfo;

const REVEAL: RouteInfo = RouteInfo::walled("/admin/reveal", &[Method::POST]);

/// The reveal route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[REVEAL];

/// The reveal route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(REVEAL.path, post(admin_reveal))
}

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
    _caller: LoopbackCaller,
    WireJson(request): WireJson<RevealRequest>,
) -> Result<StatusCode, GatewayError> {
    #[cfg(feature = "local")]
    let roots = {
        let mut roots = Vec::new();
        let config = state.config().await;
        // An unresolvable cache root (no cache_dir configured and no home
        // directory) contributes no safe root.
        if let Ok(root) = crate::local::resolve_cache_root(config.local().cache_dir()) {
            roots.push(root);
        }
        roots
    };
    #[cfg(not(feature = "local"))]
    let roots: Vec<PathBuf> = Vec::new();
    // Canonicalization and the spawn are blocking filesystem work.
    let launcher = Arc::clone(&state.reveal);
    blocking(move || {
        let command = resolve_reveal(&roots, Path::new(&request.path))?;
        launcher
            .launch(command)
            .map_err(|error| GatewayError::RevealFailed(Box::new(error)))
    })
    .await??;
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
            "path `{}` is not inside the artifact cache",
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
#[path = "reveal-tests.rs"]
mod tests;
