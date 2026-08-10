//! Gateway binary resolution and process-tree termination.
//!
//! Termination is split into a validated [`terminate`] used by the async
//! shutdown path and a best-effort [`best_effort_terminate`] used only as the
//! `Drop` fallback. Both kill the whole process tree so the gateway's
//! `llama-server` descendants never outlive the harness.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context as _, Result, bail};

use super::LOOPBACK;

/// Resolves the `promptforge-gateway` binary path.
///
/// Order: `PROMPTFORGE_GATEWAY_BIN`, then `target/debug`, then `target/release`
/// relative to the workspace (two levels above this crate's manifest).
pub(crate) fn gateway_bin() -> Result<PathBuf> {
    let override_path = std::env::var_os("PROMPTFORGE_GATEWAY_BIN").map(PathBuf::from);
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    resolve_gateway_bin(override_path.as_deref(), &workspace_target)
}

/// Testable binary resolution used by [`gateway_bin`].
fn resolve_gateway_bin(env_override: Option<&Path>, workspace_target: &Path) -> Result<PathBuf> {
    if let Some(path) = env_override {
        if path.is_file() {
            return Ok(path.to_owned());
        }
        bail!(
            "PROMPTFORGE_GATEWAY_BIN points at missing file {}",
            path.display()
        );
    }

    let name = gateway_executable_name();
    for profile in ["debug", "release"] {
        let candidate = workspace_target.join(profile).join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "promptforge-gateway binary not found under {}/{{debug,release}}/{name}; \
         build it with `cargo build -p promptforge-gateway` or set PROMPTFORGE_GATEWAY_BIN",
        workspace_target.display()
    )
}

fn gateway_executable_name() -> &'static str {
    if cfg!(windows) {
        "promptforge-gateway.exe"
    } else {
        "promptforge-gateway"
    }
}

/// Whether some listener already owns loopback `port`.
///
/// A failed bind means the port is taken (a genuine collision); a successful
/// bind means it is free, so a child that exited left the port unowned and the
/// exit had another cause.
pub(crate) fn port_has_listener(port: u16) -> bool {
    std::net::TcpListener::bind((LOOPBACK, port)).is_err()
}

/// Kills the child's process tree and reaps it, validating the outcome.
///
/// Returns an error when the child cannot be reaped, or when the platform
/// process-tree tool could not run at all (so `llama-server` descendants may
/// survive). A nonzero tool status is expected when the root already exited and
/// is not treated as a failure.
pub(crate) fn terminate(mut child: Child) -> Result<()> {
    let pid = child.id();
    // Runs inside `spawn_blocking`, so waiting on the external tool is fine here.
    let tree = kill_tree_command(pid).status();
    // An already-exited child yields `InvalidInput` from `kill`; treat only that
    // as benign and surface any other kill failure instead of discarding it.
    if let Err(error) = child.kill()
        && error.kind() != ErrorKind::InvalidInput
    {
        return Err(error).with_context(|| format!("kill promptforge-gateway child (pid {pid})"));
    }
    let status = child
        .wait()
        .with_context(|| format!("await promptforge-gateway child (pid {pid}) after kill"))?;
    let tree_status = tree.with_context(|| {
        format!("run process-tree termination for pid {pid}; descendants may survive")
    })?;
    // The child is reaped (its status observed above). The tree tool ran; a
    // nonzero status is expected when the root had already exited, so it is
    // logged for diagnosis rather than silently discarded or made fatal.
    if !tree_status.success() {
        eprintln!(
            "promptforge-gateway process-tree termination for pid {pid} exited with \
             {tree_status} (expected when the root already exited); child reaped with {status}"
        );
    }
    Ok(())
}

/// Best-effort process-tree termination for the `Drop` fallback path.
///
/// Non-blocking by construction: the external tree-kill is *spawned* (never
/// waited on) and the child is reaped with a single non-blocking `try_wait`, so
/// `Drop` can never block a runtime worker on a slow or hung `taskkill`/`kill`
/// or on a stuck descendant. Any unreaped process is collected at process exit.
/// The validated [`terminate`] is the primary, non-fallback teardown path.
pub(crate) fn best_effort_terminate(mut child: Child) {
    let _ignored = kill_tree_command(child.id()).spawn();
    let _ignored = child.kill();
    let _ignored = child.try_wait();
}

/// Builds the platform process-tree kill command targeting `pid`'s whole tree.
fn kill_tree_command(pid: u32) -> Command {
    let mut command;
    #[cfg(windows)]
    {
        command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    }
    #[cfg(unix)]
    {
        // Negative PID targets the process group started with `process_group(0)`.
        command = Command::new("kill");
        command.args(["-KILL", &format!("-{pid}")]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn resolve_gateway_bin_reports_missing_override_and_missing_target() {
        let missing = Path::new("/nonexistent/promptforge-gateway");
        let error = resolve_gateway_bin(Some(missing), Path::new("/no-such-target"))
            .expect_err("missing override must fail");
        assert!(
            format!("{error:#}").contains("PROMPTFORGE_GATEWAY_BIN"),
            "unexpected error: {error:#}"
        );

        let error = resolve_gateway_bin(None, Path::new("/no-such-target"))
            .expect_err("empty target tree must fail");
        assert!(
            format!("{error:#}").contains("promptforge-gateway binary not found"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn resolve_gateway_bin_prefers_an_existing_env_override() {
        let dir = tempfile::tempdir().expect("temp dir");
        let override_path = dir.path().join("custom-gateway");
        std::fs::write(&override_path, b"binary").expect("write override binary");
        // Executable precedence: an existing override is honored ahead of the
        // workspace target tree (empty here), and returned verbatim.
        let resolved = resolve_gateway_bin(Some(&override_path), Path::new("/no-such-target"))
            .expect("an existing override must resolve");
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn a_held_port_reports_a_listener() {
        // Deterministic and race-free: we own the listener for the whole
        // assertion, so no concurrent bind can reclaim the port under us. This
        // exercises port_has_listener's bind-fails => listener-present path. The
        // inverse (a free port is bindable => no listener) is std::net behaviour
        // and is not unit-tested here, which avoids a released-port race.
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind a held port");
        let port = listener.local_addr().expect("addr").port();
        assert!(
            port_has_listener(port),
            "a held port must report a listener"
        );
    }
}
