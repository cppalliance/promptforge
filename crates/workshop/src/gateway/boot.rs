//! Boot planning and one-shot detached Gateway launch.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{
    ConnectionFile, LaunchDecision, Resolution, SidecarError, ValidatedConnection,
};
use workshop_server::Config;

use super::identity::GatewayAttachment;

/// The sibling executable the shell launches, beside its own.
#[cfg(windows)]
pub(super) const GATEWAY_EXE_NAME: &str = "promptforge-gateway.exe";
/// The sibling executable the shell launches, beside its own.
#[cfg(not(windows))]
pub(super) const GATEWAY_EXE_NAME: &str = "promptforge-gateway";

/// Budget for the launch race and the launched Gateway readiness wait.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between polls for the launched Gateway connection file.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What the boot decision concluded.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GatewayPlan {
    /// A live connection file exists.
    Attach(ConnectionFile),
    /// No live file exists and a sibling Gateway can be launched.
    Launch(PathBuf),
    /// Explicit configuration is the only available endpoint.
    ConfigOnly,
    /// No attachment path exists.
    Fail,
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
            .and_then(validated_attachment)
            .context("launch the sidecar gateway"),
    }
}

/// Retains the selected process proof instead of reducing it to file fields.
fn validated_attachment(file: ConnectionFile) -> anyhow::Result<GatewayAttachment> {
    ValidatedConnection::validate(file)
        .map(GatewayAttachment::Sidecar)
        .context("validate the selected gateway process identity")
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
            eprintln!("could not resolve the gateway connection file: {error}");
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
fn launch_and_attach(run_dir: &Path, exe: &Path) -> anyhow::Result<ConnectionFile> {
    match shared_sidecar::launch_or_attach(run_dir, LAUNCH_TIMEOUT)
        .context("settle the gateway launch race")?
    {
        LaunchDecision::Attach(file) => Ok(file),
        LaunchDecision::Launch(lock) => {
            spawn_detached(exe).with_context(|| format!("spawn {}", exe.display()))?;
            let file = wait_for_launched_file(run_dir, LAUNCH_TIMEOUT)?;
            drop(lock);
            Ok(file)
        }
        decision => anyhow::bail!("an unknown launch decision: {decision:?}"),
    }
}

/// Waits for the launched Gateway to publish a validated connection.
fn wait_for_launched_file(run_dir: &Path, timeout: Duration) -> anyhow::Result<ConnectionFile> {
    wait_for_launched_file_with(run_dir, timeout, shared_sidecar::resolve)
}

/// Waits for readiness with resolution injected for deterministic tests.
pub(super) fn wait_for_launched_file_with<Resolve>(
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

/// Spawns the Gateway detached from the shell lifetime.
pub(super) fn spawn_detached(exe: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(
            CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    if let Err(error) = std::thread::Builder::new().spawn(move || {
        let _ = child.wait();
    }) {
        eprintln!("could not spawn the gateway reaper thread; the child goes unreaped: {error}");
    }
    Ok(())
}
