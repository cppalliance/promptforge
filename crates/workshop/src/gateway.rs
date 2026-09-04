//! Attach-or-launch: the shell's sidecar gateway lifecycle.
//!
//! Boot resolves the gateway's connection file first: a live file means a
//! gateway is already running and the shell attaches (the in-process
//! server resolves the same file). With no live file, a sibling
//! `promptforge-gateway` beside the shell's own executable is launched
//! detached - `CREATE_BREAKAWAY_FROM_JOB` on Windows so the gateway
//! survives the shell's exit and any job object; never
//! tauri-plugin-shell's sidecar API, which kills its children on exit -
//! through `shared_sidecar`'s launch lock, so two racing shells elect one
//! launcher and the loser attaches. A Workshop-only install has no
//! sibling executable: resolution falls through to explicit
//! `workshop.toml` `[gateway]` config (a LAN gateway), and with neither
//! boot fails loud naming both remedies.
//!
//! The shell never reads `gateway.toml`, never deletes `gateway.json`,
//! and never kills the gateway on exit; the quit-everything menu item
//! (`crate::menu`) is the only path that stops the gateway.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{ConnectionFile, LaunchDecision, Resolution, SidecarError};
use workshop_server::Config;

/// The sibling executable the shell launches, beside its own.
#[cfg(windows)]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway.exe";
/// The sibling executable the shell launches, beside its own.
#[cfg(not(windows))]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway";

/// The budget for each leg of the launch path: the launch race, then
/// the wait for the connection file to appear and answer health. A
/// first boot also generates the default config, so this is generous.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between polls for the launched gateway's connection file.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How boot connected the gateway: the fact the quit-everything menu
/// item labels and behaves from.
#[derive(Debug)]
pub(crate) enum GatewayAttachment {
    /// A local sidecar gateway the shell attached to or launched:
    /// quit-everything posts its `/shutdown`.
    Sidecar(ConnectionFile),
    /// An explicit-config (LAN) gateway: quit stops the shell only.
    Config,
}

impl GatewayAttachment {
    /// The connection file of a sidecar attachment, for the
    /// quit-everything shutdown post.
    pub(crate) fn sidecar_file(&self) -> Option<&ConnectionFile> {
        match self {
            Self::Sidecar(file) => Some(file),
            Self::Config => None,
        }
    }
}

/// What the boot decision concluded.
#[derive(Debug, PartialEq, Eq)]
enum GatewayPlan {
    /// A live connection file exists: attach, launch nothing.
    Attach(ConnectionFile),
    /// No live file, and a sibling gateway executable exists: launch it.
    Launch(PathBuf),
    /// No live file and no sibling executable, but explicit config: the
    /// server attaches to the configured (LAN) gateway.
    ConfigOnly,
    /// Nothing to attach to and nothing to launch: fail loud.
    Fail,
}

/// Connects the gateway for boot: attach to a live one, launch the
/// sibling executable when there is none, or fall back to explicit
/// `workshop.toml` config.
///
/// # Errors
/// Returns an error when nothing can connect (no live file, no sibling
/// executable, no explicit config) or when the launch path fails: the
/// race, the spawn, or the launched gateway never becoming healthy.
pub(crate) fn ensure_gateway(config: &Config) -> anyhow::Result<GatewayAttachment> {
    let exe_dir = std::env::current_exe()
        .context("locate the executable")
        .and_then(|exe| {
            exe.parent()
                .map(Path::to_path_buf)
                .context("the executable has no parent directory")
        })?;
    let explicit = !config.gateway.base_url.is_empty();
    let Some(run_dir) = shared_sidecar::default_run_dir() else {
        // Discovery and launch both live in the run directory; without a
        // profile directory only explicit config can connect.
        return if explicit {
            Ok(GatewayAttachment::Config)
        } else {
            Err(no_gateway_error())
        };
    };
    match plan_gateway(&run_dir, &exe_dir, explicit, shared_sidecar::resolve) {
        GatewayPlan::Attach(file) => Ok(GatewayAttachment::Sidecar(file)),
        GatewayPlan::ConfigOnly => Ok(GatewayAttachment::Config),
        GatewayPlan::Fail => Err(no_gateway_error()),
        GatewayPlan::Launch(exe) => launch_and_attach(&run_dir, &exe)
            .map(GatewayAttachment::Sidecar)
            .context("launch the sidecar gateway"),
    }
}

/// The boot decision with the environment injected: resolve the
/// connection file in `run_dir` with `resolve`, probe for the sibling
/// executable in `exe_dir`, and read whether the config names a gateway
/// explicitly.
fn plan_gateway(
    run_dir: &Path,
    exe_dir: &Path,
    explicit_config: bool,
    resolve: fn(&Path) -> Result<Resolution, SidecarError>,
) -> GatewayPlan {
    match resolve(run_dir) {
        Ok(Resolution::Attach(file)) => return GatewayPlan::Attach(file),
        Ok(_) => {}
        // An unreadable run directory must not read as "no gateway": the
        // launch lock's own re-validation settles whether a launch is
        // safe, and the config fallback matches the server's
        // degrade-on-probe-error rule.
        Err(error) => {
            eprintln!("could not resolve the gateway connection file: {error}");
        }
    }
    match sibling_gateway(exe_dir) {
        Some(exe) => GatewayPlan::Launch(exe),
        None if explicit_config => GatewayPlan::ConfigOnly,
        None => GatewayPlan::Fail,
    }
}

/// The sibling gateway executable beside the shell's own, when the
/// installer laid one down (a Gateway or full install; absent on a
/// Workshop-only install).
fn sibling_gateway(exe_dir: &Path) -> Option<PathBuf> {
    let candidate = exe_dir.join(GATEWAY_EXE_NAME);
    candidate.is_file().then_some(candidate)
}

/// The loud boot failure when nothing can connect: names both remedies.
fn no_gateway_error() -> anyhow::Error {
    anyhow::anyhow!(
        "no gateway configured or running; install the Gateway component so \
         promptforge-gateway sits beside the workshop executable, or set \
         gateway.base_url and gateway.api_key in workshop.toml to attach to \
         a gateway over the network"
    )
}

/// The launch path: take the launch lock (a racing shell attaches to the
/// winner instead), spawn the sibling gateway detached, and wait for its
/// connection file and health.
fn launch_and_attach(run_dir: &Path, exe: &Path) -> anyhow::Result<ConnectionFile> {
    match shared_sidecar::launch_or_attach(run_dir, LAUNCH_TIMEOUT)
        .context("settle the gateway launch race")?
    {
        LaunchDecision::Attach(file) => Ok(file),
        LaunchDecision::Launch(lock) => {
            spawn_detached(exe).with_context(|| format!("spawn {}", exe.display()))?;
            // The lock stays held across the wait: a racing shell
            // attaches to the file the spawn writes rather than
            // launching a second gateway. Dropping the guard releases it.
            let file = wait_for_launched_file(run_dir, LAUNCH_TIMEOUT)?;
            drop(lock);
            Ok(file)
        }
        // `LaunchDecision` is non-exhaustive; a variant this build does
        // not know fails the boot rather than guessing at it.
        decision => anyhow::bail!("an unknown launch decision: {decision:?}"),
    }
}

/// Waits for the launched gateway's connection file to appear, then for
/// its health endpoint to answer. The launch-lock holder cleaned any
/// stale file before electing a launcher, so the file that appears is
/// the new gateway's.
fn wait_for_launched_file(run_dir: &Path, timeout: Duration) -> anyhow::Result<ConnectionFile> {
    let deadline = Instant::now() + timeout;
    let file = loop {
        if let Ok(Some(file)) = ConnectionFile::read(run_dir) {
            break file;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("the launched gateway wrote no connection file within {timeout:?}");
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let url = format!("http://127.0.0.1:{}", file.port);
    shared_sidecar::wait_for_health(&url, remaining)
        .context("the launched gateway did not answer its health probe")?;
    Ok(file)
}

/// Spawns the gateway detached from the shell's lifetime: the `serve`
/// subcommand (boot discovery self-provisions the config on first run),
/// silent stdio, and on Windows broken out of any job object with no
/// console of its own, so the gateway survives the shell's exit.
fn spawn_detached(exe: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // Break out of any job object whose kill-on-close would reap the
        // gateway with the shell, take no console (the gateway is a
        // console-subsystem binary spawned from a GUI process), and leave
        // the shell's Ctrl-C group.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(
            CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
        );
    }
    // A new process group keeps a terminal's Ctrl-C (SIGINT to the
    // shell's group) from reaching the gateway.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    // Reap the child when it eventually exits; a detached child nobody
    // waits on lingers as a zombie for the shell's whole lifetime. When
    // the reaper thread cannot be spawned the child goes unreaped - the
    // benign outcome the thread exists to prevent - so report and boot
    // on rather than panic out of the designed loud boot-failure path.
    if let Err(error) = std::thread::Builder::new().spawn(move || {
        let _ = child.wait();
    }) {
        eprintln!("could not spawn the gateway reaper thread; the child goes unreaped: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;

    /// The test process's own image name, so the probe's pid and image
    /// checks pass and the test reaches the liveness probes.
    fn own_image_name() -> String {
        std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("the exe has a file name")
            .to_string_lossy()
            .into_owned()
    }

    /// A probe running the real liveness gauntlet against the test
    /// binary's own image.
    fn probe_own_image(run_dir: &Path) -> Result<Resolution, SidecarError> {
        shared_sidecar::resolve_for_test(run_dir, &own_image_name())
    }

    /// A probe whose run directory is broken: a directory sits where the
    /// connection file belongs, so the read errors instead of answering.
    fn probe_read_failure(run_dir: &Path) -> Result<Resolution, SidecarError> {
        std::fs::create_dir(shared_sidecar::connection_file_path(run_dir))
            .expect("plant the unreadable file");
        probe_own_image(run_dir)
    }

    /// A connection file pointing at the test process itself.
    fn live_file(port: u16, api_key: &str) -> ConnectionFile {
        ConnectionFile {
            port,
            api_key: api_key.to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    /// A pid guaranteed dead: a short-lived child, reaped and dropped so
    /// no handle keeps the process object alive.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        pid
    }

    /// A fixture gateway: answers `GET /health` with 200 and the key
    /// probe with 200 only when the bearer matches `expected_key`.
    fn fixture_gateway(expected_key: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let Ok(read) = stream.read(&mut buffer) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let response = if request.starts_with("GET /health ")
                    || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"))
                {
                    &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                } else {
                    &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                };
                let _ = stream.write_all(response);
            }
        });
        port
    }

    /// An executable directory, with or without the sibling gateway.
    fn exe_dir(with_gateway: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        if with_gateway {
            std::fs::write(dir.path().join(GATEWAY_EXE_NAME), b"").expect("plant the sibling exe");
        }
        let path = dir.path().to_owned();
        (dir, path)
    }

    #[test]
    fn a_live_file_attaches_without_looking_for_a_sibling_exe() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("key");
        let file = live_file(port, "key");
        file.write_to(run.path()).expect("write");
        // No sibling exe and no explicit config: attach must not consult
        // either.
        let (_exe, exe_dir) = exe_dir(false);

        match plan_gateway(run.path(), &exe_dir, false, probe_own_image) {
            GatewayPlan::Attach(attached) => assert_eq!(attached, file),
            other => panic!("a live gateway must be attached, not {other:?}"),
        }
    }

    #[test]
    fn no_file_and_a_sibling_exe_launches() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let (_exe, exe_dir) = exe_dir(true);

        match plan_gateway(run.path(), &exe_dir, false, probe_own_image) {
            GatewayPlan::Launch(exe) => assert_eq!(exe, exe_dir.join(GATEWAY_EXE_NAME)),
            other => panic!("a full install with no running gateway must launch, not {other:?}"),
        }
    }

    #[test]
    fn no_file_and_no_sibling_exe_falls_through_to_explicit_config() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let (_exe, exe_dir) = exe_dir(false);

        let plan = plan_gateway(run.path(), &exe_dir, true, probe_own_image);
        assert_eq!(
            plan,
            GatewayPlan::ConfigOnly,
            "a Workshop-only install attaches to the configured LAN gateway"
        );
    }

    #[test]
    fn no_file_no_sibling_exe_and_no_config_fails() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let (_exe, exe_dir) = exe_dir(false);

        let plan = plan_gateway(run.path(), &exe_dir, false, probe_own_image);
        assert_eq!(
            plan,
            GatewayPlan::Fail,
            "nothing to connect to must fail loud, not serve a broken window"
        );
    }

    #[test]
    fn a_stale_file_is_cleaned_and_the_sibling_exe_launches() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..live_file(1, "k")
        };
        file.write_to(run.path()).expect("write");
        let (_exe, exe_dir) = exe_dir(true);

        let plan = plan_gateway(run.path(), &exe_dir, false, probe_own_image);
        assert!(
            matches!(plan, GatewayPlan::Launch(_)),
            "a stale file must not block the relaunch: {plan:?}"
        );
        assert!(
            !shared_sidecar::connection_file_path(run.path()).exists(),
            "the stale file was cleaned"
        );
    }

    #[test]
    fn a_stale_file_with_no_sibling_exe_falls_through_to_explicit_config() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..live_file(1, "k")
        };
        file.write_to(run.path()).expect("write");
        let (_exe, exe_dir) = exe_dir(false);

        let plan = plan_gateway(run.path(), &exe_dir, true, probe_own_image);
        assert_eq!(
            plan,
            GatewayPlan::ConfigOnly,
            "a stale file must not wedge the LAN fallback"
        );
        assert!(
            !shared_sidecar::connection_file_path(run.path()).exists(),
            "the stale file was cleaned"
        );
    }

    #[test]
    fn a_resolve_error_still_launches_the_sibling_exe() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let (_exe, exe_dir) = exe_dir(true);

        let plan = plan_gateway(run.path(), &exe_dir, false, probe_read_failure);
        assert!(
            matches!(plan, GatewayPlan::Launch(_)),
            "a discovery error must not read as no-gateway: {plan:?}"
        );
    }

    #[test]
    fn the_sibling_probe_finds_only_the_gateway_exe_beside_the_shell() {
        let (_dir, with) = exe_dir(true);
        assert_eq!(
            sibling_gateway(&with),
            Some(with.join(GATEWAY_EXE_NAME)),
            "the installed sibling is found"
        );
        let (_dir, without) = exe_dir(false);
        assert_eq!(
            sibling_gateway(&without),
            None,
            "a Workshop-only install has no sibling"
        );
    }

    #[test]
    fn the_launch_wait_returns_once_the_file_appears_and_answers() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("key");
        let file = live_file(port, "key");
        let run_path = run.path().to_owned();
        let written = file.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            written
                .write_to(&run_path)
                .expect("the launched gateway writes");
        });

        let waited = wait_for_launched_file(run.path(), Duration::from_secs(5))
            .expect("the file lands and answers");
        assert_eq!(waited, file);
        writer.join().expect("the writer thread ran");
    }

    #[test]
    fn the_launch_wait_times_out_when_no_file_appears() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let error = wait_for_launched_file(run.path(), Duration::from_millis(150))
            .expect_err("a gateway that never writes must not hang boot");
        assert!(
            error.to_string().contains("no connection file"),
            "the error names the missing file: {error}"
        );
    }

    #[test]
    fn an_explicit_config_attachment_holds_no_file_for_the_shutdown_post() {
        let file = live_file(1, "k");
        let sidecar = GatewayAttachment::Sidecar(file.clone());
        assert_eq!(
            sidecar.sidecar_file(),
            Some(&file),
            "a sidecar attachment hands the quit item the connection file"
        );
        let config = GatewayAttachment::Config;
        assert_eq!(
            config.sidecar_file(),
            None,
            "a LAN gateway from explicit config never gets a shutdown post"
        );
    }

    #[test]
    fn the_no_gateway_error_names_both_remedies() {
        let message = no_gateway_error().to_string();
        assert!(
            message.contains("promptforge-gateway"),
            "the error names the Gateway component remedy: {message}"
        );
        assert!(
            message.contains("workshop.toml"),
            "the error names the explicit-config remedy: {message}"
        );
    }
}
