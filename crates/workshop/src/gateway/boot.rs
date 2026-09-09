//! Boot planning and one-shot detached Gateway launch.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{
    GatewayDiscoveryFile, LaunchDecision, Resolution, SidecarError, ValidatedConnection,
};
use workshop_server::Config;

use super::identity::GatewayAttachment;
use super::supervisor::{RecoveryCandidate, RecoveryOwnership};

/// The sibling executable the shell launches, beside its own.
#[cfg(windows)]
pub(super) const GATEWAY_EXE_NAME: &str = "promptforge-gateway.exe";
/// The sibling executable the shell launches, beside its own.
#[cfg(not(windows))]
pub(super) const GATEWAY_EXE_NAME: &str = "promptforge-gateway";

/// Budget for the launch race and the launched Gateway readiness wait.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between polls for the launched Gateway discovery file.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const WINDOWS_DETACHED_CREATION_FLAGS: u32 = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS;
#[cfg(windows)]
const WINDOWS_BREAKAWAY_CREATION_FLAGS: u32 =
    CREATE_BREAKAWAY_FROM_JOB | WINDOWS_DETACHED_CREATION_FLAGS;

/// What the boot decision concluded.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GatewayPlan {
    /// A live gateway discovery file exists.
    Attach(GatewayDiscoveryFile),
    /// No live file exists and a sibling Gateway can be launched.
    Launch(PathBuf),
    /// Explicit configuration is the only available endpoint.
    ConfigOnly,
    /// No attachment path exists.
    Fail,
}

/// The result of a launch election, retaining the spawned pid only for the
/// branch that actually created a child.
#[derive(Debug)]
pub(super) enum RecoveryLaunch {
    /// A race winner had already published a live Gateway.
    Attached(GatewayDiscoveryFile),
    /// This process spawned a child and observed a gateway discovery file.
    Launched {
        child_pid: u32,
        file: GatewayDiscoveryFile,
    },
}

/// Connects the Gateway for boot.
///
/// # Errors
/// Returns an error when no attachment path exists or launch fails.
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
        return if explicit {
            Ok(GatewayAttachment::Config)
        } else {
            Err(no_gateway_error())
        };
    };
    match plan_gateway(&run_dir, &exe_dir, explicit, shared_sidecar::resolve) {
        GatewayPlan::Attach(file) => validated_attachment(file),
        GatewayPlan::ConfigOnly => Ok(GatewayAttachment::Config),
        GatewayPlan::Fail => Err(no_gateway_error()),
        GatewayPlan::Launch(exe) => launch_and_attach(&run_dir, &exe)
            .and_then(validated_recovery_attachment)
            .context("launch the sidecar gateway"),
    }
}

/// Retains the selected process proof instead of reducing it to file fields.
fn validated_attachment(file: GatewayDiscoveryFile) -> anyhow::Result<GatewayAttachment> {
    ValidatedConnection::validate(file)
        .map(GatewayAttachment::Sidecar)
        .context("validate the selected gateway process identity")
}

fn validated_recovery_attachment(recovery: RecoveryLaunch) -> anyhow::Result<GatewayAttachment> {
    match recovery {
        RecoveryLaunch::Attached(file) => validated_attachment(file),
        RecoveryLaunch::Launched { child_pid, file } => {
            let validated = ValidatedConnection::validate(file)
                .context("validate the launched gateway process identity")?;
            Ok(
                match RecoveryCandidate::authenticate(child_pid, validated) {
                    RecoveryOwnership::Owned(candidate) => GatewayAttachment::Launched(candidate),
                    RecoveryOwnership::Unowned(unowned) => GatewayAttachment::Sidecar(unowned),
                },
            )
        }
    }
}

/// Chooses attach, launch, configured fallback, or failure.
pub(super) fn plan_gateway(
    run_dir: &Path,
    exe_dir: &Path,
    explicit_config: bool,
    resolve: fn(&Path) -> Result<Resolution, SidecarError>,
) -> GatewayPlan {
    match resolve(run_dir) {
        Ok(Resolution::Attach(file)) => return GatewayPlan::Attach(file),
        Ok(_) => {}
        Err(error) => {
            eprintln!("could not resolve the gateway discovery file: {error}");
        }
    }
    match sibling_gateway(exe_dir) {
        Some(exe) => GatewayPlan::Launch(exe),
        None if explicit_config => GatewayPlan::ConfigOnly,
        None => GatewayPlan::Fail,
    }
}

/// Locates the installed sibling Gateway executable.
pub(super) fn sibling_gateway(exe_dir: &Path) -> Option<PathBuf> {
    let candidate = exe_dir.join(GATEWAY_EXE_NAME);
    candidate.is_file().then_some(candidate)
}

/// Builds the loud boot failure naming both supported remedies.
pub(super) fn no_gateway_error() -> anyhow::Error {
    anyhow::anyhow!(
        "no gateway configured or running; install the Gateway component so \
         promptforge-gateway sits beside the workshop executable, or set \
         gateway.base_url and gateway.api_key in workshop.toml to attach to \
         a gateway over the network"
    )
}

/// Settles the launch race, launches once when elected, and attaches.
fn launch_and_attach(run_dir: &Path, exe: &Path) -> anyhow::Result<RecoveryLaunch> {
    match shared_sidecar::launch_or_attach(run_dir, LAUNCH_TIMEOUT)
        .context("settle the gateway launch race")?
    {
        LaunchDecision::Attach(file) => Ok(RecoveryLaunch::Attached(file)),
        LaunchDecision::Launch(lock) => {
            let child_pid =
                spawn_detached(exe).with_context(|| format!("spawn {}", exe.display()))?;
            let file = wait_for_launched_file(run_dir, LAUNCH_TIMEOUT)?;
            drop(lock);
            Ok(RecoveryLaunch::Launched { child_pid, file })
        }
        decision => anyhow::bail!("an unknown launch decision: {decision:?}"),
    }
}

/// Waits for the launched Gateway to publish a validated connection.
fn wait_for_launched_file(
    run_dir: &Path,
    timeout: Duration,
) -> anyhow::Result<GatewayDiscoveryFile> {
    wait_for_launched_file_with(run_dir, timeout, shared_sidecar::resolve)
}

/// Waits for readiness with resolution injected for deterministic tests.
pub(super) fn wait_for_launched_file_with<Resolve>(
    run_dir: &Path,
    timeout: Duration,
    mut resolve: Resolve,
) -> anyhow::Result<GatewayDiscoveryFile>
where
    Resolve: FnMut(&Path) -> Result<Resolution, SidecarError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(file)) = GatewayDiscoveryFile::read(run_dir) {
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
                "the launched gateway wrote no validated gateway discovery file within {timeout:?}"
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(windows)]
pub(super) fn spawn_detached_windows_with<T, Spawn>(mut spawn: Spawn) -> std::io::Result<T>
where
    Spawn: FnMut(u32) -> std::io::Result<T>,
{
    match spawn(WINDOWS_BREAKAWAY_CREATION_FLAGS) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            // Hosted runners can forbid breakaway from their job object.
            // https://github.com/actions/runner/issues/595
            spawn(WINDOWS_DETACHED_CREATION_FLAGS)
        }
        result => result,
    }
}

fn detached_command(exe: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// Spawns the Gateway detached from the shell lifetime.
pub(super) fn spawn_detached(exe: &Path) -> std::io::Result<u32> {
    #[cfg(windows)]
    let mut child = spawn_detached_windows_with(|flags| {
        use std::os::windows::process::CommandExt as _;
        detached_command(exe).creation_flags(flags).spawn()
    })?;
    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt as _;
        let mut command = detached_command(exe);
        command.process_group(0);
        command.spawn()?
    };
    #[cfg(not(any(windows, unix)))]
    let mut child = detached_command(exe).spawn()?;
    let child_pid = child.id();
    if let Err(error) = std::thread::Builder::new().spawn(move || {
        let _ = child.wait();
    }) {
        eprintln!("could not spawn the gateway reaper thread; the child goes unreaped: {error}");
    }
    Ok(child_pid)
}
