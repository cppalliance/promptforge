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
//! (`crate::menu`) is the only path that stops the Gateway, through the
//! server's current validated binding snapshot.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{
    CancellationToken, ConnectionFile, LaunchDecision, Resolution, SidecarError,
    ValidatedConnection,
};
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

/// Healthy-sidecar supervision cadence.
const SUPERVISION_INTERVAL: Duration = Duration::from_secs(5);

/// First delay after a failed re-resolution or relaunch.
const SUPERVISION_BASE_DELAY: Duration = Duration::from_millis(250);

/// Ceiling on repeated sidecar recovery attempts.
const SUPERVISION_MAX_DELAY: Duration = Duration::from_secs(30);

/// Maximum designed supervisor shutdown latency.
const SUPERVISOR_SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);

/// One sidecar liveness observation.
enum SupervisionProbe {
    /// Another process already published a live replacement.
    Replacement(ConnectionFile),
    /// No live local Gateway is currently discoverable.
    Missing,
}

/// The running local-sidecar supervisor.
#[derive(Debug)]
pub(crate) struct GatewaySupervisor {
    cancellation: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GatewaySupervisor {
    fn spawn(supervise: impl FnOnce(CancellationToken) + Send + 'static) -> anyhow::Result<Self> {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let thread = std::thread::Builder::new()
            .name("gateway-supervisor".to_owned())
            .spawn(move || supervise(worker_cancellation))
            .context("spawn the gateway supervisor")?;
        Ok(Self {
            cancellation,
            thread: Some(thread),
        })
    }

    /// Cancels supervision and joins its thread.
    pub(crate) fn shutdown(mut self) {
        self.cancel_and_join();
    }

    fn cancel_and_join(&mut self) {
        let started = Instant::now();
        self.cancellation.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("the gateway supervisor panicked during shutdown");
        }
        let elapsed = started.elapsed();
        if elapsed > SUPERVISOR_SHUTDOWN_BUDGET {
            eprintln!(
                "the gateway supervisor exceeded its {SUPERVISOR_SHUTDOWN_BUDGET:?} shutdown budget: {elapsed:?}"
            );
        }
    }
}

impl Drop for GatewaySupervisor {
    fn drop(&mut self) {
        self.cancel_and_join();
    }
}

/// How boot connected the Gateway: the fact the quit-everything menu labels
/// from and the supervisor uses to decide whether it owns local recovery.
#[derive(Debug)]
pub(crate) enum GatewayAttachment {
    /// A local sidecar gateway the shell attached to or launched:
    /// quit-everything posts its `/shutdown`.
    Sidecar(ConnectionFile),
    /// An explicit-config (LAN) gateway: quit stops the shell only.
    Config,
}

impl GatewayAttachment {
    /// The initial connection file of a sidecar attachment.
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

fn launch_and_attach_cancellable(
    run_dir: &Path,
    exe: &Path,
    cancellation: &CancellationToken,
) -> anyhow::Result<ConnectionFile> {
    launch_and_attach_cancellable_with(
        run_dir,
        exe,
        cancellation,
        shared_sidecar::launch_or_attach_cancellable,
        |exe, _| spawn_detached(exe),
        wait_for_launched_file_cancellable,
    )
}

fn launch_and_attach_cancellable_with<Settle, Spawn, Wait>(
    run_dir: &Path,
    exe: &Path,
    cancellation: &CancellationToken,
    settle: Settle,
    spawn: Spawn,
    wait: Wait,
) -> anyhow::Result<ConnectionFile>
where
    Settle: FnOnce(&Path, Duration, &CancellationToken) -> Result<LaunchDecision, SidecarError>,
    Spawn: FnOnce(&Path, &CancellationToken) -> std::io::Result<()>,
    Wait: FnOnce(&Path, Duration, &CancellationToken) -> anyhow::Result<ConnectionFile>,
{
    match settle(run_dir, LAUNCH_TIMEOUT, cancellation).context("settle the gateway launch race")? {
        LaunchDecision::Attach(file) => {
            if cancellation.is_cancelled() {
                anyhow::bail!("gateway attachment was cancelled");
            }
            Ok(file)
        }
        LaunchDecision::Launch(lock) => {
            run_effect_if_active(cancellation, "gateway launch", |cancellation| {
                if cancellation.is_cancelled() {
                    anyhow::bail!("gateway launch was cancelled");
                }
                spawn(exe, cancellation).with_context(|| format!("spawn {}", exe.display()))
            })?;
            let file = wait(run_dir, LAUNCH_TIMEOUT, cancellation)?;
            drop(lock);
            Ok(file)
        }
        decision => anyhow::bail!("an unknown launch decision: {decision:?}"),
    }
}

fn run_effect_if_active<T>(
    cancellation: &CancellationToken,
    phase: &'static str,
    operation: impl FnOnce(&CancellationToken) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match cancellation.run_if_active(|| operation(cancellation)) {
        Some(result) => result,
        None => anyhow::bail!("{phase} was cancelled"),
    }
}

/// Waits for the launched gateway's connection file to appear, become ready,
/// and pass the shared process-image, health, and bearer validation.
fn wait_for_launched_file(run_dir: &Path, timeout: Duration) -> anyhow::Result<ConnectionFile> {
    wait_for_launched_file_with(run_dir, timeout, shared_sidecar::resolve)
}

/// Waits for readiness, then accepts only a connection file that passes the
/// shared process-image, health, and bearer validation.
fn wait_for_launched_file_with<Resolve>(
    run_dir: &Path,
    timeout: Duration,
    mut resolve: Resolve,
) -> anyhow::Result<ConnectionFile>
where
    Resolve: FnMut(&Path) -> Result<Resolution, SidecarError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(file)) = ConnectionFile::read(run_dir) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let url = format!("http://127.0.0.1:{}", file.port);
            shared_sidecar::wait_for_health(&url, remaining)
                .context("the launched gateway did not answer its health probe")?;
            if let Ok(Resolution::Attach(validated)) = resolve(run_dir) {
                return Ok(validated);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "the launched gateway wrote no validated connection file within {timeout:?}"
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_launched_file_cancellable(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> anyhow::Result<ConnectionFile> {
    wait_for_launched_file_cancellable_with(
        run_dir,
        timeout,
        cancellation,
        shared_sidecar::wait_for_health_cancellable,
        shared_sidecar::resolve_cancellable,
    )
}

fn wait_for_launched_file_cancellable_with<Health, Resolve>(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
    mut health: Health,
    mut resolve: Resolve,
) -> anyhow::Result<ConnectionFile>
where
    Health: FnMut(&str, Duration, &CancellationToken) -> Result<(), shared_sidecar::HealthError>,
    Resolve: FnMut(&Path, &CancellationToken) -> Result<Resolution, shared_sidecar::SidecarError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if let Ok(Some(file)) = ConnectionFile::read(run_dir) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let url = format!("http://127.0.0.1:{}", file.port);
            health(&url, remaining, cancellation)
                .context("the launched gateway did not answer its health probe")?;
            if cancellation.is_cancelled() {
                anyhow::bail!("the launched gateway wait was cancelled");
            }
            match resolve(run_dir, cancellation) {
                Ok(Resolution::Attach(validated)) => {
                    if cancellation.is_cancelled() {
                        anyhow::bail!("the launched gateway wait was cancelled");
                    }
                    return Ok(validated);
                }
                Err(SidecarError::Cancelled) => {
                    anyhow::bail!("the launched gateway wait was cancelled");
                }
                Ok(_) | Err(_) => {}
            }
        }
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "the launched gateway wrote no validated connection file within {timeout:?}"
            );
        }
        if cancellation.wait_timeout(POLL_INTERVAL) {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
    }
}

/// Spawns the gateway detached from the shell's lifetime: the bare
/// invocation serves (boot discovery self-provisions the config on first
/// run), silent stdio, and on Windows broken out of any job object with no
/// console of its own, so the gateway survives the shell's exit.
fn spawn_detached(exe: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
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

fn validate_and_publish_with<T>(
    file: &ConnectionFile,
    cancellation: &CancellationToken,
    validate: impl FnOnce(&ConnectionFile, &CancellationToken) -> anyhow::Result<T>,
    publish: impl FnOnce(&T, &CancellationToken) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let validated = validate(file, cancellation)?;
    if cancellation.is_cancelled() {
        anyhow::bail!("gateway publication was cancelled");
    }
    publish(&validated, cancellation)
}

/// Starts runtime supervision only for a connection-file sidecar.
///
/// Explicitly configured LAN endpoints return `None`: their address is fixed,
/// and this process neither probes them for replacement nor launches anything.
pub(crate) fn supervise(
    attachment: &GatewayAttachment,
    updater: workshop_server::GatewayUpdater,
) -> anyhow::Result<Option<GatewaySupervisor>> {
    let Some(initial) = attachment.sidecar_file().cloned() else {
        return Ok(None);
    };
    let run_dir = shared_sidecar::default_run_dir().context("locate the sidecar run directory")?;
    let exe_dir = std::env::current_exe()
        .context("locate the executable")?
        .parent()
        .map(Path::to_path_buf)
        .context("the executable has no parent directory")?;
    let sibling = sibling_gateway(&exe_dir);
    GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            initial,
            |_, cancellation| match shared_sidecar::resolve_cancellable(&run_dir, cancellation) {
                Ok(Resolution::Attach(file)) => SupervisionProbe::Replacement(file),
                Ok(_) | Err(SidecarError::Cancelled) => SupervisionProbe::Missing,
                Err(error) => {
                    eprintln!("could not re-resolve the local gateway: {error}");
                    SupervisionProbe::Missing
                }
            },
            |cancellation| {
                let exe = sibling.as_deref().context(
                    "the local gateway disappeared and no sibling gateway executable is installed",
                )?;
                launch_and_attach_cancellable(&run_dir, exe, cancellation)
            },
            |file, cancellation| {
                validate_and_publish_with(
                    file,
                    cancellation,
                    |file, cancellation| {
                        ValidatedConnection::validate_cancellable(file.clone(), cancellation)
                            .context("validate the replacement gateway identity")
                    },
                    |validated, cancellation| {
                        if updater
                            .replace_sidecar_cancellable(validated, cancellation)
                            .context("publish the replacement gateway endpoint")?
                        {
                            Ok(())
                        } else {
                            anyhow::bail!("gateway publication was cancelled")
                        }
                    },
                )
            },
            |delay, cancellation| cancellation.wait_timeout(delay),
            &cancellation,
        );
    })
    .map(Some)
}

/// Runs the supervision state machine with I/O injected for deterministic
/// liveness and recovery tests.
fn run_supervision<Probe, Recover, Publish, Wait, Error>(
    mut current: ConnectionFile,
    mut probe: Probe,
    mut recover: Recover,
    mut publish: Publish,
    mut wait: Wait,
    cancellation: &CancellationToken,
) where
    Probe: FnMut(&ConnectionFile, &CancellationToken) -> SupervisionProbe,
    Recover: FnMut(&CancellationToken) -> Result<ConnectionFile, Error>,
    Publish: FnMut(&ConnectionFile, &CancellationToken) -> Result<(), Error>,
    Wait: FnMut(Duration, &CancellationToken) -> bool,
    Error: std::fmt::Display,
{
    let mut retry_delay = SUPERVISION_BASE_DELAY;
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        let observation = probe(&current, cancellation);
        if cancellation.is_cancelled() {
            return;
        }
        match observation {
            SupervisionProbe::Replacement(file) if same_gateway_identity(&file, &current) => {
                retry_delay = SUPERVISION_BASE_DELAY;
                if wait(SUPERVISION_INTERVAL, cancellation) {
                    return;
                }
                continue;
            }
            SupervisionProbe::Replacement(file) => match publish(&file, cancellation) {
                Ok(()) => {
                    if cancellation.is_cancelled() {
                        return;
                    }
                    current = file;
                    retry_delay = SUPERVISION_BASE_DELAY;
                    continue;
                }
                Err(error) => {
                    eprintln!("could not publish a replacement local gateway: {error}");
                }
            },
            SupervisionProbe::Missing => match recover(cancellation) {
                Ok(_) if cancellation.is_cancelled() => return,
                Ok(file) => match publish(&file, cancellation) {
                    Ok(()) => {
                        if cancellation.is_cancelled() {
                            return;
                        }
                        current = file;
                        retry_delay = SUPERVISION_BASE_DELAY;
                        continue;
                    }
                    Err(error) => {
                        eprintln!("could not publish a replacement local gateway: {error}");
                    }
                },
                Err(error) => {
                    eprintln!("could not recover the local gateway: {error}");
                }
            },
        }
        if cancellation.is_cancelled() || wait(retry_delay, cancellation) {
            return;
        }
        retry_delay = retry_delay.saturating_mul(2).min(SUPERVISION_MAX_DELAY);
    }
}

/// Whether two validated connection files describe the same Gateway boot.
fn same_gateway_identity(left: &ConnectionFile, right: &ConnectionFile) -> bool {
    left.pid == right.pid && left.epoch == right.epoch && left.started_at == right.started_at
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

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
    fn fixture_gateway(expected_key: impl Into<String>) -> u16 {
        let expected_key = Arc::new(expected_key.into());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let expected_key = Arc::clone(&expected_key);
                std::thread::spawn(move || {
                    loop {
                        let mut buffer = [0u8; 1024];
                        let Ok(read) = stream.read(&mut buffer) else {
                            break;
                        };
                        if read == 0 {
                            break;
                        }
                        let request = String::from_utf8_lossy(&buffer[..read]);
                        let accepted = request.starts_with("GET /health ")
                            || request.lines().any(|line| {
                                line.eq_ignore_ascii_case(&format!(
                                    "Authorization: Bearer {expected_key}"
                                ))
                            });
                        let response = if accepted {
                            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                        } else {
                            &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                        };
                        if stream.write_all(response).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        port
    }

    const CONTROL_ADDRESS_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_CONTROL_ADDRESS";
    const EXPECTED_KEY_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_EXPECTED_KEY";

    struct NamedGateway {
        child: Child,
        port: u16,
        _directory: tempfile::TempDir,
    }

    impl NamedGateway {
        fn spawn(expected_key: &str) -> Self {
            let control = TcpListener::bind("127.0.0.1:0").expect("bind fixture control");
            control
                .set_nonblocking(true)
                .expect("make fixture control nonblocking");
            let directory = tempfile::TempDir::new().expect("create fixture executable directory");
            let executable = directory.path().join(GATEWAY_EXE_NAME);
            std::fs::copy(
                std::env::current_exe().expect("locate test executable"),
                &executable,
            )
            .expect("copy test executable under the Gateway image name");
            let mut child = Command::new(&executable)
                .args([
                    "--exact",
                    "gateway::tests::validated_gateway_fixture_process",
                    "--ignored",
                ])
                .env(
                    CONTROL_ADDRESS_ENV,
                    control
                        .local_addr()
                        .expect("read fixture control address")
                        .to_string(),
                )
                .env(EXPECTED_KEY_ENV, expected_key)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start the named Gateway fixture process");
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match control.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            child.try_wait().expect("observe fixture process").is_none(),
                            "the named Gateway fixture exited before becoming ready"
                        );
                        assert!(
                            Instant::now() < deadline,
                            "the named Gateway fixture did not become ready"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept fixture control connection: {error}"),
                }
            };
            let mut port = [0_u8; 2];
            stream
                .read_exact(&mut port)
                .expect("read fixture Gateway port");
            Self {
                child,
                port: u16::from_be_bytes(port),
                _directory: directory,
            }
        }

        fn connection_file(&self, api_key: &str, epoch: u64, started_at: &str) -> ConnectionFile {
            ConnectionFile {
                port: self.port,
                api_key: api_key.to_owned(),
                pid: self.child.id(),
                epoch,
                version: "test".to_owned(),
                started_at: started_at.to_owned(),
            }
        }
    }

    impl Drop for NamedGateway {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    #[ignore = "runs only as a named child process"]
    fn validated_gateway_fixture_process() {
        let Ok(control_address) = std::env::var(CONTROL_ADDRESS_ENV) else {
            return;
        };
        let expected_key =
            std::env::var(EXPECTED_KEY_ENV).expect("the fixture child receives an expected key");
        let port = fixture_gateway(expected_key);
        TcpStream::connect(control_address)
            .and_then(|mut stream| stream.write_all(&port.to_be_bytes()))
            .expect("announce named fixture readiness");
        loop {
            std::thread::park();
        }
    }

    fn get(url: &str, path: &str) -> String {
        let url = url::Url::parse(url).expect("the Workshop URL parses");
        let host = url.host_str().expect("the Workshop URL has a host");
        let port = url.port().expect("the Workshop URL has a port");
        let mut stream = TcpStream::connect((host, port)).expect("connect to Workshop");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
        )
        .expect("send Workshop request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read Workshop response");
        response
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

        let waited =
            wait_for_launched_file_with(run.path(), Duration::from_secs(5), probe_own_image)
                .expect("the validated file lands and answers");
        assert_eq!(waited, file);
        writer.join().expect("the writer thread ran");
    }

    #[test]
    fn the_launch_wait_times_out_when_no_file_appears() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let error =
            wait_for_launched_file_with(run.path(), Duration::from_millis(150), probe_own_image)
                .expect_err("a gateway that never writes must not hang boot");
        assert!(
            error.to_string().contains("no validated connection file"),
            "the error names the missing file: {error}"
        );
    }

    #[test]
    fn the_launch_wait_rejects_a_key_the_live_process_does_not_accept() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let file = live_file(fixture_gateway("accepted-key"), "rejected-key");
        file.write_to(run.path()).expect("write");

        let error =
            wait_for_launched_file_with(run.path(), Duration::from_millis(150), probe_own_image)
                .expect_err("an unaccepted connection-file key must not publish");

        assert!(
            error.to_string().contains("no validated connection file"),
            "the error names the validation failure without exposing the key: {error}"
        );
    }

    #[test]
    fn an_explicit_config_attachment_holds_no_local_sidecar_file() {
        let file = live_file(1, "k");
        let sidecar = GatewayAttachment::Sidecar(file.clone());
        assert_eq!(
            sidecar.sidecar_file(),
            Some(&file),
            "a sidecar attachment carries its initial local connection file"
        );
        let config = GatewayAttachment::Config;
        assert_eq!(
            config.sidecar_file(),
            None,
            "a LAN Gateway from explicit config carries no local sidecar identity"
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

    #[test]
    fn supervision_lives_past_sixty_seconds_then_propagates_a_configured_key_edit_atomically() {
        use std::cell::{Cell, RefCell};

        let gateway = NamedGateway::spawn("new-key");
        let original = gateway.connection_file("old-key", 1_757_000_000, "2026-09-03T12:00:00Z");
        let replacement = gateway.connection_file("new-key", 1_757_000_001, "2026-09-03T12:00:01Z");
        let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
        let server = workshop_server::fixtures::spawn(workshop_server::Config {
            gateway: workshop_server::GatewayConfig {
                base_url: format!("http://127.0.0.1:{}", original.port),
                api_key: original.api_key.clone(),
            },
            server: workshop_server::ServerConfig {
                bind: "127.0.0.1:0".to_owned(),
                open_browser: false,
                state_dir: state_dir.path().to_owned(),
            },
            agents: workshop_server::AgentsConfig::default(),
        })
        .expect("spawn Workshop against the original same-port key");
        let updater = server.gateway_updater();
        let publish_replacement = |file: &ConnectionFile| -> anyhow::Result<()> {
            let validated = ValidatedConnection::validate(file.clone())
                .context("validate the named local Gateway")?;
            updater
                .replace_sidecar(&validated)
                .context("publish through the production updater")
        };
        let elapsed = Cell::new(Duration::ZERO);
        let recoveries = Cell::new(0_u8);
        let published = RefCell::new(Vec::new());
        let cancellation = CancellationToken::new();

        run_supervision(
            original.clone(),
            |current, _| {
                if !published.borrow().is_empty() || elapsed.get() <= Duration::from_secs(65) {
                    SupervisionProbe::Replacement(current.clone())
                } else {
                    SupervisionProbe::Missing
                }
            },
            |_| {
                recoveries.set(recoveries.get() + 1);
                if recoveries.get() < 3 {
                    anyhow::bail!("injected launch failure");
                }
                Ok(replacement.clone())
            },
            |file, _| {
                publish_replacement(file)?;
                published.borrow_mut().push(file.clone());
                Ok::<(), anyhow::Error>(())
            },
            |delay, _| {
                assert!(
                    delay <= SUPERVISION_MAX_DELAY,
                    "every supervision wait is capped: {delay:?}"
                );
                elapsed.set(elapsed.get() + delay);
                !published.borrow().is_empty()
            },
            &cancellation,
        );

        assert!(
            elapsed.get() > Duration::from_secs(60),
            "the supervisor remains live beyond one minute"
        );
        assert_eq!(recoveries.get(), 3, "failed launches retry under backoff");
        assert_eq!(
            published.borrow().as_slice(),
            [replacement],
            "one successful relaunch publishes its exact connection-file pair"
        );
        assert_eq!(
            published.borrow()[0].port,
            original.port,
            "an OS-assigned port may be reused"
        );
        assert_ne!(
            published.borrow()[0].api_key,
            original.api_key,
            "a configured key edit propagates with the replacement identity"
        );
        let response = get(server.url(), "/gateway/api/admin/status");
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "the real publisher replaces the bearer on the reused port: {response}"
        );
        server.shutdown().expect("stop the Workshop fixture");
    }

    #[test]
    fn a_new_pid_replacement_publishes_even_when_port_and_key_are_unchanged() {
        use std::cell::RefCell;

        let original = live_file(54_375, "stable-key");
        let replacement = ConnectionFile {
            pid: original.pid + 1,
            epoch: original.epoch + 1,
            started_at: "2026-09-03T12:00:01Z".to_owned(),
            ..original.clone()
        };
        let published = RefCell::new(Vec::new());
        let cancellation = CancellationToken::new();

        run_supervision(
            original.clone(),
            |current, _| {
                if published.borrow().is_empty() {
                    SupervisionProbe::Replacement(replacement.clone())
                } else {
                    SupervisionProbe::Replacement(current.clone())
                }
            },
            |_| -> anyhow::Result<ConnectionFile> {
                panic!("a validated replacement does not need a relaunch")
            },
            |file, _| {
                published.borrow_mut().push(file.clone());
                Ok(())
            },
            |_, _| !published.borrow().is_empty(),
            &cancellation,
        );

        assert_eq!(
            published.borrow().as_slice(),
            [replacement],
            "new process identity publishes the exact stable endpoint and credential pair"
        );
        assert_eq!(published.borrow()[0].port, original.port);
        assert_eq!(published.borrow()[0].api_key, original.api_key);
    }

    #[test]
    fn pid_or_boot_identity_distinguishes_replacement_from_endpoint_changes() {
        let original = live_file(54_375, "stable-key");
        let new_pid = ConnectionFile {
            pid: original.pid + 1,
            ..original.clone()
        };
        let new_boot = ConnectionFile {
            epoch: original.epoch + 1,
            started_at: "2026-09-03T12:00:01Z".to_owned(),
            ..original.clone()
        };
        let endpoint_only = ConnectionFile {
            port: 54_379,
            api_key: "edited-without-restart".to_owned(),
            ..original.clone()
        };

        assert!(!same_gateway_identity(&original, &new_pid));
        assert!(!same_gateway_identity(&original, &new_boot));
        assert!(same_gateway_identity(&original, &endpoint_only));
    }

    fn assert_bounded_supervisor_shutdown(supervisor: GatewaySupervisor, finished: &AtomicBool) {
        let started = Instant::now();
        supervisor.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "Workshop exit joins the cancelled supervisor within its budget"
        );
        assert!(
            finished.load(Ordering::SeqCst),
            "shutdown returns only after the supervisor thread exits"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_in_resolve_before_recovery() {
        let (entered, blocked) = mpsc::channel();
        let recoveries = Arc::new(AtomicUsize::new(0));
        let worker_recoveries = Arc::clone(&recoveries);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            run_supervision(
                live_file(54_375, "stable-key"),
                |_, cancellation| {
                    entered.send(()).expect("announce blocked resolve");
                    let _ = cancellation.wait_timeout(Duration::from_secs(30));
                    SupervisionProbe::Missing
                },
                |_| {
                    worker_recoveries.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("recovery must not start after cancellation")
                },
                |_, _| Ok::<(), anyhow::Error>(()),
                |delay, cancellation| cancellation.wait_timeout(delay),
                &cancellation,
            );
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the resolve phase blocks deterministically");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            recoveries.load(Ordering::SeqCst),
            0,
            "cancellation prevents every later recovery launch"
        );
    }

    #[test]
    fn exit_wakes_the_supervision_wait_without_a_later_probe() {
        let (entered, blocked) = mpsc::channel();
        let probes = Arc::new(AtomicUsize::new(0));
        let worker_probes = Arc::clone(&probes);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            run_supervision(
                live_file(54_375, "stable-key"),
                |current, _| {
                    worker_probes.fetch_add(1, Ordering::SeqCst);
                    SupervisionProbe::Replacement(current.clone())
                },
                |_| -> anyhow::Result<ConnectionFile> {
                    panic!("a healthy Gateway does not recover")
                },
                |_, _| Ok::<(), anyhow::Error>(()),
                |delay, cancellation| {
                    entered.send(()).expect("announce supervision wait");
                    cancellation.wait_timeout(delay)
                },
                &cancellation,
            );
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the supervision wait blocks deterministically");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "cancellation prevents every later liveness probe"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_in_validation_before_publication() {
        let (entered, blocked) = mpsc::channel();
        let publications = Arc::new(AtomicUsize::new(0));
        let worker_publications = Arc::clone(&publications);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            let result = validate_and_publish_with(
                &live_file(54_375, "stable-key"),
                &cancellation,
                |_, cancellation| -> anyhow::Result<()> {
                    entered.send(()).expect("announce blocked validation");
                    let _ = cancellation.wait_timeout(Duration::from_secs(30));
                    anyhow::bail!("validation cancelled")
                },
                |(), _| {
                    worker_publications.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            );
            assert!(result.is_err(), "the cancelled validation is rejected");
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the validation phase blocks deterministically");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            publications.load(Ordering::SeqCst),
            0,
            "cancelled validation cannot publish into the snapshot"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_in_health_wait_before_resolve() {
        let run = tempfile::TempDir::new().expect("tempdir");
        live_file(54_375, "stable-key")
            .write_to(run.path())
            .expect("write candidate");
        let run_dir = run.path().to_owned();
        let (entered, blocked) = mpsc::channel();
        let resolves = Arc::new(AtomicUsize::new(0));
        let worker_resolves = Arc::clone(&resolves);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            let result = wait_for_launched_file_cancellable_with(
                &run_dir,
                Duration::from_secs(30),
                &cancellation,
                |_, _, cancellation| {
                    entered.send(()).expect("announce blocked health wait");
                    let _ = cancellation.wait_timeout(Duration::from_secs(30));
                    Err(shared_sidecar::HealthError::Cancelled)
                },
                |_, _| {
                    worker_resolves.fetch_add(1, Ordering::SeqCst);
                    Ok(Resolution::Absent)
                },
            );
            assert!(result.is_err(), "the cancelled health wait is rejected");
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the health-wait phase blocks deterministically");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            resolves.load(Ordering::SeqCst),
            0,
            "cancelled health waiting cannot start a later resolve"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_in_launch_race_before_spawn() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let decision = shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
            .expect("acquire a launch decision");
        let run_dir = run.path().to_owned();
        let (entered, blocked) = mpsc::channel();
        let launches = Arc::new(AtomicUsize::new(0));
        let worker_launches = Arc::clone(&launches);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            let mut decision = Some(decision);
            let result = launch_and_attach_cancellable_with(
                &run_dir,
                Path::new("unused-gateway"),
                &cancellation,
                |_, _, cancellation| {
                    entered.send(()).expect("announce blocked launch race");
                    let _ = cancellation.wait_timeout(Duration::from_secs(30));
                    Ok(decision.take().expect("one launch decision"))
                },
                |_, _| {
                    worker_launches.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |_, _, _| anyhow::bail!("the cancelled launch cannot wait for health"),
            );
            assert!(result.is_err(), "the cancelled launch race is rejected");
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the launch race blocks deterministically");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            launches.load(Ordering::SeqCst),
            0,
            "cancelled launch racing cannot create a process"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_inside_launch_without_process_creation() {
        let run = tempfile::TempDir::new().expect("tempdir");
        let decision = shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
            .expect("acquire a launch decision");
        let run_dir = run.path().to_owned();
        let (entered, blocked) = mpsc::channel();
        let launches = Arc::new(AtomicUsize::new(0));
        let worker_launches = Arc::clone(&launches);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            let mut decision = Some(decision);
            let result = launch_and_attach_cancellable_with(
                &run_dir,
                Path::new("unused-gateway"),
                &cancellation,
                |_, _, _| Ok(decision.take().expect("one launch decision")),
                |_, cancellation| {
                    entered.send(()).expect("announce blocked launch");
                    if cancellation.wait_timeout(Duration::from_secs(30)) {
                        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                    }
                    worker_launches.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |_, _, _| anyhow::bail!("the cancelled launch cannot wait for health"),
            );
            assert!(result.is_err(), "the cancelled launch is rejected");
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the process launch blocks deterministically inside its effect gate");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            launches.load(Ordering::SeqCst),
            0,
            "the cancelled launch has no post-cancel effect"
        );
    }

    #[test]
    fn exit_joins_a_supervisor_blocked_inside_publication_without_replacement() {
        let (entered, blocked) = mpsc::channel();
        let publications = Arc::new(AtomicUsize::new(0));
        let worker_publications = Arc::clone(&publications);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let supervisor = GatewaySupervisor::spawn(move |cancellation| {
            let result = validate_and_publish_with(
                &live_file(54_375, "stable-key"),
                &cancellation,
                |_, _| Ok(()),
                |(), cancellation| {
                    run_effect_if_active(cancellation, "gateway publication", |cancellation| {
                        entered.send(()).expect("announce blocked publication");
                        if cancellation.wait_timeout(Duration::from_secs(30)) {
                            anyhow::bail!("gateway publication was cancelled");
                        }
                        worker_publications.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                },
            );
            assert!(result.is_err(), "the cancelled publication is rejected");
            worker_finished.store(true, Ordering::SeqCst);
        })
        .expect("spawn test supervisor");
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot publication blocks deterministically inside its effect gate");

        assert_bounded_supervisor_shutdown(supervisor, &finished);
        assert_eq!(
            publications.load(Ordering::SeqCst),
            0,
            "cancellation prevents authoritative snapshot replacement"
        );
    }
}
