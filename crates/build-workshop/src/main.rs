//! Builds the PromptForge Gateway, stages it for Tauri, builds Workshop,
//! and removes the temporary staged sidecar.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{Child, ExitCode};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

#[path = "main/pipeline.rs"]
mod pipeline;
#[path = "main/runner.rs"]
mod runner;

use pipeline::build_workshop;
use runner::install_interrupt_handler;

const USAGE: &str = "\
Build PromptForge Gateway and Workshop together.

USAGE:
    cargo workshop [--release] [--target <triple>]

OPTIONS:
    --release           Build both products with Cargo's release profile
    --target <triple>   Build both products for this target triple
    -h, --help          Print this help
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Profile {
    Debug,
    Release,
}

impl Profile {
    fn directory(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct BuildRequest {
    profile: Profile,
    target: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Capture,
    Inherit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandSpec {
    program: PathBuf,
    args: Vec<OsString>,
    current_dir: PathBuf,
    output_mode: OutputMode,
}

#[derive(Debug, Eq, PartialEq)]
struct CommandResult {
    success: bool,
    stdout: String,
    stderr: String,
}

trait CommandRunner {
    fn run(
        &mut self,
        command: &CommandSpec,
        interrupt_policy: InterruptPolicy,
    ) -> io::Result<CommandResult>;

    fn last_command_started(&self) -> bool;

    fn interruption_observed(&self) -> bool;
}

#[derive(Debug)]
struct ProcessRunner {
    interrupt: InterruptController,
    last_command_started: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptPolicy {
    RejectExisting,
    AllowExisting,
}

#[derive(Clone, Debug)]
struct InterruptController {
    state: Arc<InterruptState>,
}

#[derive(Debug)]
struct InterruptState {
    generation: AtomicUsize,
    active_child: Mutex<Option<Child>>,
    termination_error: Mutex<Option<String>>,
}

#[derive(Debug)]
struct BuildEnvironment {
    workspace_root: PathBuf,
    target_root: PathBuf,
    cargo: PathBuf,
    node: PathBuf,
}

impl BuildEnvironment {
    fn discover() -> Result<Self, anyhow::Error> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(|crates| crates.parent())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot derive the workspace root from {}",
                    manifest_dir.display()
                )
            })?
            .to_path_buf();
        let target_root = match std::env::var_os("CARGO_TARGET_DIR") {
            Some(value) if value.is_empty() => {
                return Err(anyhow::anyhow!("CARGO_TARGET_DIR must not be empty"));
            }
            Some(value) => {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map_err(|error| {
                            anyhow::anyhow!("cannot read the current directory: {error}")
                        })?
                        .join(path)
                }
            }
            None => workspace_root.join("target"),
        };
        let cargo = std::env::var_os("CARGO")
            .or_else(|| option_env!("CARGO").map(OsString::from))
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!("Cargo did not provide the executable used for this command")
            })?;
        Ok(Self {
            workspace_root,
            target_root,
            cargo,
            node: PathBuf::from("node"),
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct BuildError {
    primary: String,
    cleanup: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct StepError {
    message: String,
    interrupted: bool,
    command_started: bool,
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.primary)?;
        if let Some(cleanup) = &self.cleanup {
            write!(formatter, "\ncleanup also failed: {cleanup}")?;
        }
        Ok(())
    }
}

fn parse_arguments(args: &[String]) -> Result<BuildRequest, anyhow::Error> {
    let mut profile = Profile::Debug;
    let mut release_seen = false;
    let mut target = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--release" if release_seen => {
                return Err(anyhow::anyhow!("duplicate argument `--release`\n\n{USAGE}"));
            }
            "--release" => {
                profile = Profile::Release;
                release_seen = true;
                index += 1;
            }
            "--target" if target.is_some() => {
                return Err(anyhow::anyhow!("duplicate argument `--target`\n\n{USAGE}"));
            }
            "--target" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!("argument `--target` needs a target triple\n\n{USAGE}")
                })?;
                if value.starts_with('-') || !valid_target_triple(value) {
                    return Err(anyhow::anyhow!(
                        "argument `--target` needs a valid target triple, got `{value}`\n\n{USAGE}"
                    ));
                }
                target = Some(value.clone());
                index += 2;
            }
            argument => {
                return Err(anyhow::anyhow!(
                    "unsupported argument `{argument}`\n\n{USAGE}"
                ));
            }
        }
    }
    Ok(BuildRequest { profile, target })
}

fn valid_target_triple(target: &str) -> bool {
    let parts: Vec<&str> = target.split('-').collect();
    parts.len() >= 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [argument] if argument == "-h" || argument == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let request = match parse_arguments(&args) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let environment = match BuildEnvironment::discover() {
        Ok(environment) => environment,
        Err(error) => {
            eprintln!("build-workshop failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let interrupt = match install_interrupt_handler() {
        Ok(interrupt) => interrupt,
        Err(error) => {
            eprintln!("build-workshop failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match build_workshop(&request, &environment, &mut ProcessRunner::new(interrupt)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("build-workshop failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
