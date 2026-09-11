//! Builds the PromptForge Gateway, stages it for Tauri, builds Workshop,
//! and removes the temporary staged sidecar.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read as _};
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

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

impl InterruptController {
    fn isolated() -> Self {
        Self {
            state: Arc::new(InterruptState {
                generation: AtomicUsize::new(0),
                active_child: Mutex::new(None),
                termination_error: Mutex::new(None),
            }),
        }
    }

    fn generation(&self) -> usize {
        self.state.generation.load(Ordering::SeqCst)
    }

    fn request(&self) {
        let _ =
            self.state
                .generation
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |generation| {
                    Some(generation.saturating_add(1))
                });
        let mut active_child = self.lock_active_child();
        if let Some(child) = active_child.as_mut()
            && let Err(error) = child.kill()
            && error.kind() != io::ErrorKind::InvalidInput
        {
            let mut termination_error = self.lock_termination_error();
            if termination_error.is_none() {
                *termination_error = Some(error.to_string());
            }
        }
    }

    fn lock_active_child(&self) -> MutexGuard<'_, Option<Child>> {
        match self.state.active_child.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn lock_termination_error(&self) -> MutexGuard<'_, Option<String>> {
        match self.state.termination_error.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn clear_termination_error(&self) {
        *self.lock_termination_error() = None;
    }

    fn take_termination_error(&self) -> Option<String> {
        self.lock_termination_error().take()
    }
}

static PROCESS_INTERRUPT: OnceLock<Result<InterruptController, Arc<anyhow::Error>>> =
    OnceLock::new();

fn install_interrupt_handler() -> Result<InterruptController, Arc<anyhow::Error>> {
    match PROCESS_INTERRUPT.get_or_init(|| {
        let interrupt = InterruptController::isolated();
        let handler_interrupt = interrupt.clone();
        ctrlc::set_handler(move || handler_interrupt.request()).map_err(|error| {
            Arc::new(anyhow::anyhow!(
                "cannot install the interrupt handler: {error}"
            ))
        })?;
        Ok(interrupt)
    }) {
        Ok(interrupt) => Ok(interrupt.clone()),
        Err(error) => Err(Arc::clone(error)),
    }
}

impl ProcessRunner {
    fn new(interrupt: InterruptController) -> Self {
        Self {
            interrupt,
            last_command_started: false,
        }
    }

    fn interrupted_error(&self) -> io::Error {
        match self.interrupt.take_termination_error() {
            Some(error) => io::Error::new(
                io::ErrorKind::Interrupted,
                format!("interrupted; active child termination failed: {error}"),
            ),
            None => io::Error::new(io::ErrorKind::Interrupted, "interrupted"),
        }
    }

    fn finish_child(
        mut child: Child,
        status: ExitStatus,
        output_mode: OutputMode,
    ) -> io::Result<CommandResult> {
        let mut stdout = String::new();
        let mut stderr = String::new();
        if output_mode == OutputMode::Capture {
            if let Some(mut pipe) = child.stdout.take() {
                pipe.read_to_string(&mut stdout)?;
            }
            if let Some(mut pipe) = child.stderr.take() {
                pipe.read_to_string(&mut stderr)?;
            }
        }
        Ok(CommandResult {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}

impl CommandRunner for ProcessRunner {
    fn run(
        &mut self,
        command: &CommandSpec,
        interrupt_policy: InterruptPolicy,
    ) -> io::Result<CommandResult> {
        self.last_command_started = false;
        let generation = self.interrupt.generation();
        if interrupt_policy == InterruptPolicy::RejectExisting && generation != 0 {
            return Err(self.interrupted_error());
        }

        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .current_dir(&command.current_dir);
        if command.output_mode == OutputMode::Capture {
            process.stdout(Stdio::piped()).stderr(Stdio::piped());
        }

        {
            let mut active_child = self.interrupt.lock_active_child();
            if self.interrupt.generation() != generation {
                return Err(self.interrupted_error());
            }
            if active_child.is_some() {
                return Err(io::Error::other(
                    "another build subprocess is already active",
                ));
            }
            self.interrupt.clear_termination_error();
            *active_child = Some(process.spawn()?);
            self.last_command_started = true;
        }

        loop {
            let completed = {
                let mut active_child = self.interrupt.lock_active_child();
                let child = active_child
                    .as_mut()
                    .ok_or_else(|| io::Error::other("active build subprocess disappeared"))?;
                match child.try_wait()? {
                    Some(status) => active_child.take().map(|child| (child, status)),
                    None => None,
                }
            };
            if let Some((child, status)) = completed {
                let result = Self::finish_child(child, status, command.output_mode)?;
                if self.interrupt.generation() != generation {
                    return Err(self.interrupted_error());
                }
                return Ok(result);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn last_command_started(&self) -> bool {
        self.last_command_started
    }

    fn interruption_observed(&self) -> bool {
        self.interrupt.generation() != 0
    }
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

fn build_workshop(
    request: &BuildRequest,
    environment: &BuildEnvironment,
    runner: &mut impl CommandRunner,
) -> Result<(), BuildError> {
    let target = match &request.target {
        Some(target) => target.clone(),
        None => discover_host_target(environment, runner).map_err(|error| BuildError {
            primary: error.message,
            cleanup: None,
        })?,
    };
    let mut staging_started = false;
    let primary = build_products(request, &target, environment, runner, &mut staging_started);
    let mut primary = match primary {
        Err(error) if error.interrupted && !staging_started => {
            return Err(BuildError {
                primary: error.message,
                cleanup: None,
            });
        }
        result => result,
    };
    if primary.is_ok() && runner.interruption_observed() {
        primary = Err(StepError {
            message: "Workshop build interrupted after child completion".to_owned(),
            interrupted: true,
            command_started: true,
        });
    }
    let cleanup = run_checked(
        runner,
        &sidecar_command(environment, "remove", &target, None),
        "Gateway sidecar cleanup",
        InterruptPolicy::AllowExisting,
    )
    .map(|_| ());
    if primary.is_ok() && cleanup.is_ok() && runner.interruption_observed() {
        primary = Err(StepError {
            message: "Workshop build interrupted after child completion".to_owned(),
            interrupted: true,
            command_started: true,
        });
    }

    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(BuildError {
            primary: error.message,
            cleanup: None,
        }),
        (Err(primary), Err(cleanup)) => Err(BuildError {
            primary: primary.message,
            cleanup: Some(cleanup.message),
        }),
    }
}

fn discover_host_target(
    environment: &BuildEnvironment,
    runner: &mut impl CommandRunner,
) -> Result<String, StepError> {
    let output = run_checked(
        runner,
        &CommandSpec {
            program: environment.cargo.clone(),
            args: vec![OsString::from("-vV")],
            current_dir: environment.workspace_root.clone(),
            output_mode: OutputMode::Capture,
        },
        "Cargo host discovery",
        InterruptPolicy::RejectExisting,
    )?;
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .filter(|target| valid_target_triple(target))
        .map(str::to_owned)
        .ok_or_else(|| StepError {
            message: "Cargo host triple was absent or malformed in `cargo -vV` output".to_owned(),
            interrupted: false,
            command_started: true,
        })
}

fn build_products(
    request: &BuildRequest,
    target: &str,
    environment: &BuildEnvironment,
    runner: &mut impl CommandRunner,
    staging_started: &mut bool,
) -> Result<(), StepError> {
    run_checked(
        runner,
        &cargo_build_command(environment, request, "gateway"),
        "Gateway build",
        InterruptPolicy::RejectExisting,
    )?;

    let mut source = environment.target_root.clone();
    if request.target.is_some() {
        source.push(target);
    }
    source.push(request.profile.directory());
    source.push(gateway_binary_name(target));

    match run_checked(
        runner,
        &sidecar_command(environment, "stage", target, Some(source)),
        "Gateway sidecar staging",
        InterruptPolicy::RejectExisting,
    ) {
        Ok(_) => *staging_started = true,
        Err(error) => {
            *staging_started = error.command_started;
            return Err(error);
        }
    }
    run_checked(
        runner,
        &cargo_build_command(environment, request, "workshop"),
        "Workshop build",
        InterruptPolicy::RejectExisting,
    )?;
    Ok(())
}

fn cargo_build_command(
    environment: &BuildEnvironment,
    request: &BuildRequest,
    package: &str,
) -> CommandSpec {
    let mut args = vec![
        OsString::from("build"),
        OsString::from("-p"),
        OsString::from(package),
    ];
    if request.profile == Profile::Release {
        args.push(OsString::from("--release"));
    }
    if let Some(target) = &request.target {
        args.push(OsString::from("--target"));
        args.push(OsString::from(target));
    }
    CommandSpec {
        program: environment.cargo.clone(),
        args,
        current_dir: environment.workspace_root.clone(),
        output_mode: OutputMode::Inherit,
    }
}

fn sidecar_command(
    environment: &BuildEnvironment,
    action: &str,
    target: &str,
    source: Option<PathBuf>,
) -> CommandSpec {
    let mut args = vec![
        environment
            .workspace_root
            .join("tools")
            .join("stage-gateway-sidecar.mjs")
            .into_os_string(),
        OsString::from(action),
        OsString::from("--target"),
        OsString::from(target),
    ];
    if let Some(source) = source {
        args.push(OsString::from("--source"));
        args.push(source.into_os_string());
    }
    CommandSpec {
        program: environment.node.clone(),
        args,
        current_dir: environment.workspace_root.clone(),
        output_mode: OutputMode::Inherit,
    }
}

fn gateway_binary_name(target: &str) -> &'static str {
    if target.split('-').any(|part| part == "windows") {
        "promptforge-gateway.exe"
    } else {
        "promptforge-gateway"
    }
}

fn run_checked(
    runner: &mut impl CommandRunner,
    command: &CommandSpec,
    label: &str,
    interrupt_policy: InterruptPolicy,
) -> Result<String, StepError> {
    let result = runner.run(command, interrupt_policy);
    let command_started = runner.last_command_started();
    let result = result.map_err(|error| {
        if error.kind() == io::ErrorKind::Interrupted {
            StepError {
                message: format!("{label} interrupted: {error}"),
                interrupted: true,
                command_started,
            }
        } else {
            StepError {
                message: format!("{label} could not start: {error}"),
                interrupted: false,
                command_started,
            }
        }
    })?;
    if result.success {
        return Ok(result.stdout);
    }
    let detail = if result.stderr.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", result.stderr.trim())
    };
    Err(StepError {
        message: format!("{label} failed{detail}"),
        interrupted: false,
        command_started,
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
mod tests {
    use std::collections::VecDeque;

    use tempfile::TempDir;

    use super::*;

    #[derive(Debug)]
    enum FakeResponse {
        Completed(CommandResult),
        CompletedAndInterrupted,
        InterruptedBeforeStart,
        InterruptedAfterStart,
        SpawnFailed(io::ErrorKind, &'static str),
    }

    #[derive(Debug, Default)]
    struct FakeRunner {
        responses: VecDeque<FakeResponse>,
        commands: Vec<CommandSpec>,
        last_command_started: bool,
        interruption_observed: bool,
    }

    impl FakeRunner {
        fn with_responses(responses: Vec<FakeResponse>) -> Self {
            Self {
                responses: responses.into(),
                commands: Vec::new(),
                last_command_started: false,
                interruption_observed: false,
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &mut self,
            command: &CommandSpec,
            interrupt_policy: InterruptPolicy,
        ) -> io::Result<CommandResult> {
            self.commands.push(command.clone());
            self.last_command_started = false;
            if interrupt_policy == InterruptPolicy::RejectExisting && self.interruption_observed {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "interrupted before child start",
                ));
            }
            match self.responses.pop_front().expect("unexpected command") {
                FakeResponse::Completed(result) => {
                    self.last_command_started = true;
                    Ok(result)
                }
                FakeResponse::CompletedAndInterrupted => {
                    self.last_command_started = true;
                    self.interruption_observed = true;
                    Ok(CommandResult {
                        success: true,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }
                FakeResponse::InterruptedBeforeStart => {
                    self.interruption_observed = true;
                    Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"))
                }
                FakeResponse::InterruptedAfterStart => {
                    self.last_command_started = true;
                    self.interruption_observed = true;
                    Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"))
                }
                FakeResponse::SpawnFailed(kind, message) => Err(io::Error::new(kind, message)),
            }
        }

        fn last_command_started(&self) -> bool {
            self.last_command_started
        }

        fn interruption_observed(&self) -> bool {
            self.interruption_observed
        }
    }

    fn success(stdout: &str) -> FakeResponse {
        FakeResponse::Completed(CommandResult {
            success: true,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    fn failure(stderr: &str) -> FakeResponse {
        FakeResponse::Completed(CommandResult {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        })
    }

    struct TestEnvironment {
        _temp: TempDir,
        environment: BuildEnvironment,
    }

    fn environment() -> TestEnvironment {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let workspace_root = temp.path().join("repo");
        let target_root = temp.path().join("cargo-target");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        TestEnvironment {
            _temp: temp,
            environment: BuildEnvironment {
                workspace_root,
                target_root,
                cargo: PathBuf::from("selected-cargo"),
                node: PathBuf::from("selected-node"),
            },
        }
    }

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn command(
        environment: &BuildEnvironment,
        program: &str,
        args: &[&str],
        output_mode: OutputMode,
    ) -> CommandSpec {
        CommandSpec {
            program: PathBuf::from(program),
            args: strings(args),
            current_dir: environment.workspace_root.clone(),
            output_mode,
        }
    }

    #[test]
    fn parses_only_the_documented_profile_and_target_options() {
        assert_eq!(
            parse_arguments(&[]).expect("debug request"),
            BuildRequest {
                profile: Profile::Debug,
                target: None,
            }
        );
        assert_eq!(
            parse_arguments(&[
                "--target".to_owned(),
                "aarch64-apple-darwin".to_owned(),
                "--release".to_owned(),
            ])
            .expect("release target request"),
            BuildRequest {
                profile: Profile::Release,
                target: Some("aarch64-apple-darwin".to_owned()),
            }
        );
    }

    #[test]
    fn rejects_product_features_and_other_unsupported_arguments() {
        for args in [
            vec!["--features".to_owned(), "local".to_owned()],
            vec!["--profile".to_owned(), "dist".to_owned()],
            vec!["gateway".to_owned()],
        ] {
            let error = parse_arguments(&args)
                .expect_err("unsupported argument")
                .to_string();
            assert!(error.contains("unsupported argument"), "{error}");
            assert!(error.contains(USAGE), "{error}");
        }
    }

    #[test]
    fn rejects_duplicate_or_incomplete_options() {
        for args in [
            vec!["--release".to_owned(), "--release".to_owned()],
            vec!["--target".to_owned()],
            vec![
                "--target".to_owned(),
                "x86_64-pc-windows-msvc".to_owned(),
                "--target".to_owned(),
                "x86_64-unknown-linux-gnu".to_owned(),
            ],
        ] {
            assert!(parse_arguments(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn default_build_derives_host_and_cleans_up_in_order() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner = FakeRunner::with_responses(vec![
            success("cargo 1.89.0\nhost: x86_64-pc-windows-msvc\n"),
            success(""),
            success(""),
            success(""),
            success(""),
        ]);

        build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: None,
            },
            environment,
            &mut runner,
        )
        .expect("Workshop build");

        let source = environment
            .target_root
            .join("debug")
            .join("promptforge-gateway.exe");
        assert_eq!(
            runner.commands,
            vec![
                command(environment, "selected-cargo", &["-vV"], OutputMode::Capture),
                command(
                    environment,
                    "selected-cargo",
                    &["build", "-p", "gateway"],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-node",
                    &[
                        environment
                            .workspace_root
                            .join("tools")
                            .join("stage-gateway-sidecar.mjs")
                            .to_str()
                            .expect("UTF-8 script"),
                        "stage",
                        "--target",
                        "x86_64-pc-windows-msvc",
                        "--source",
                        source.to_str().expect("UTF-8 source"),
                    ],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-cargo",
                    &["build", "-p", "workshop"],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-node",
                    &[
                        environment
                            .workspace_root
                            .join("tools")
                            .join("stage-gateway-sidecar.mjs")
                            .to_str()
                            .expect("UTF-8 script"),
                        "remove",
                        "--target",
                        "x86_64-pc-windows-msvc",
                    ],
                    OutputMode::Inherit,
                ),
            ]
        );
    }

    #[test]
    fn explicit_release_target_uses_target_output_without_a_host_probe() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "aarch64-unknown-linux-gnu";
        let mut runner =
            FakeRunner::with_responses(vec![success(""), success(""), success(""), success("")]);

        build_workshop(
            &BuildRequest {
                profile: Profile::Release,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect("Workshop build");

        let source = environment
            .target_root
            .join(triple)
            .join("release")
            .join("promptforge-gateway");
        assert_eq!(
            runner.commands,
            vec![
                command(
                    environment,
                    "selected-cargo",
                    &["build", "-p", "gateway", "--release", "--target", triple,],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-node",
                    &[
                        environment
                            .workspace_root
                            .join("tools")
                            .join("stage-gateway-sidecar.mjs")
                            .to_str()
                            .expect("UTF-8 script"),
                        "stage",
                        "--target",
                        triple,
                        "--source",
                        source.to_str().expect("UTF-8 source"),
                    ],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-cargo",
                    &["build", "-p", "workshop", "--release", "--target", triple,],
                    OutputMode::Inherit,
                ),
                command(
                    environment,
                    "selected-node",
                    &[
                        environment
                            .workspace_root
                            .join("tools")
                            .join("stage-gateway-sidecar.mjs")
                            .to_str()
                            .expect("UTF-8 script"),
                        "remove",
                        "--target",
                        triple,
                    ],
                    OutputMode::Inherit,
                ),
            ]
        );
    }

    #[test]
    fn gateway_failure_still_removes_any_staged_sidecar() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "x86_64-pc-windows-msvc";
        let mut runner = FakeRunner::with_responses(vec![failure("gateway broke"), success("")]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("Gateway failure");

        assert!(error.primary.contains("Gateway build failed"), "{error}");
        assert!(error.primary.contains("gateway broke"), "{error}");
        assert_eq!(error.cleanup, None);
        assert_eq!(runner.commands.len(), 2);
        assert_eq!(
            runner.commands[1],
            command(
                environment,
                "selected-node",
                &[
                    environment
                        .workspace_root
                        .join("tools")
                        .join("stage-gateway-sidecar.mjs")
                        .to_str()
                        .expect("UTF-8 script"),
                    "remove",
                    "--target",
                    triple,
                ],
                OutputMode::Inherit,
            )
        );
    }

    #[test]
    fn workshop_failure_is_primary_when_cleanup_also_fails() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "x86_64-unknown-linux-gnu";
        let mut runner = FakeRunner::with_responses(vec![
            success(""),
            success(""),
            failure("workshop broke"),
            FakeResponse::SpawnFailed(io::ErrorKind::NotFound, "node disappeared"),
        ]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Release,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("Workshop and cleanup failure");

        assert!(error.primary.contains("Workshop build failed"), "{error}");
        assert!(error.primary.contains("workshop broke"), "{error}");
        let cleanup = error.cleanup.expect("separate cleanup failure");
        assert!(cleanup.contains("Gateway sidecar cleanup"), "{cleanup}");
        assert!(cleanup.contains("node disappeared"), "{cleanup}");
    }

    #[test]
    fn stage_failure_is_followed_by_cleanup() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner =
            FakeRunner::with_responses(vec![success(""), failure("copy failed"), success("")]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some("x86_64-unknown-linux-gnu".to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("staging failure");

        assert!(error.primary.contains("Gateway sidecar staging failed"));
        assert_eq!(runner.commands.len(), 3);
        assert_eq!(
            runner.commands[2].args[1..],
            strings(&["remove", "--target", "x86_64-unknown-linux-gnu"])
        );
    }

    #[test]
    fn interruption_before_staging_does_not_run_removal() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner = FakeRunner::with_responses(vec![FakeResponse::InterruptedBeforeStart]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some("x86_64-pc-windows-msvc".to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("Gateway interruption");

        assert!(
            error.primary.contains("Gateway build interrupted"),
            "{error}"
        );
        assert_eq!(error.cleanup, None);
        assert_eq!(runner.commands.len(), 1);
    }

    #[test]
    fn interruption_after_gateway_completion_does_not_run_removal() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "x86_64-pc-windows-msvc";
        let mut runner = FakeRunner::with_responses(vec![FakeResponse::CompletedAndInterrupted]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("pre-staging interruption");

        assert!(
            error
                .primary
                .contains("Gateway sidecar staging interrupted"),
            "{error}"
        );
        assert!(
            runner
                .commands
                .iter()
                .all(|command| { command.args[1..] != strings(&["remove", "--target", triple]) })
        );
    }

    #[test]
    fn interruption_during_staging_runs_target_cleanup_once() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "x86_64-pc-windows-msvc";
        let mut runner = FakeRunner::with_responses(vec![
            success(""),
            FakeResponse::InterruptedAfterStart,
            success(""),
        ]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("staging interruption");

        assert!(
            error
                .primary
                .contains("Gateway sidecar staging interrupted"),
            "{error}"
        );
        let removals = runner
            .commands
            .iter()
            .filter(|command| command.args[1..] == strings(&["remove", "--target", triple]))
            .count();
        assert_eq!(removals, 1);
    }

    #[test]
    fn interruption_raced_with_completion_runs_cleanup_once() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let triple = "x86_64-pc-windows-msvc";
        let mut runner = FakeRunner::with_responses(vec![
            success(""),
            success(""),
            FakeResponse::CompletedAndInterrupted,
            success(""),
        ]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some(triple.to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("Workshop completion race");

        assert!(
            error.primary.contains("Workshop build interrupted"),
            "{error}"
        );
        let removals = runner
            .commands
            .iter()
            .filter(|command| command.args[1..] == strings(&["remove", "--target", triple]))
            .count();
        assert_eq!(removals, 1);
    }

    #[test]
    fn interruption_preserves_cleanup_failure_diagnostics() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner = FakeRunner::with_responses(vec![
            success(""),
            success(""),
            FakeResponse::InterruptedAfterStart,
            failure("remove broke"),
        ]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some("x86_64-unknown-linux-gnu".to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("Workshop interruption and cleanup failure");

        assert!(
            error.primary.contains("Workshop build interrupted"),
            "{error}"
        );
        let cleanup = error.cleanup.expect("cleanup diagnostic");
        assert!(
            cleanup.contains("Gateway sidecar cleanup failed"),
            "{cleanup}"
        );
        assert!(cleanup.contains("remove broke"), "{cleanup}");
    }

    #[test]
    fn cleanup_failure_after_success_fails_the_command_clearly() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner = FakeRunner::with_responses(vec![
            success(""),
            success(""),
            success(""),
            failure("remove failed"),
        ]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: Some("x86_64-unknown-linux-gnu".to_owned()),
            },
            environment,
            &mut runner,
        )
        .expect_err("cleanup failure");

        assert!(
            error.primary.contains("Gateway sidecar cleanup failed"),
            "{error}"
        );
        assert!(error.primary.contains("remove failed"), "{error}");
        assert_eq!(error.cleanup, None);
    }

    #[test]
    fn malformed_host_output_fails_before_building() {
        let test_environment = environment();
        let environment = &test_environment.environment;
        let mut runner = FakeRunner::with_responses(vec![success("cargo 1.89.0\n")]);

        let error = build_workshop(
            &BuildRequest {
                profile: Profile::Debug,
                target: None,
            },
            environment,
            &mut runner,
        )
        .expect_err("missing host");

        assert!(error.primary.contains("Cargo host triple"), "{error}");
        assert_eq!(runner.commands.len(), 1);
    }
}
