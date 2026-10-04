//! The build pipeline: host discovery, product builds, sidecar staging, and cleanup.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use super::{
    BuildEnvironment, BuildError, BuildRequest, CommandRunner, CommandSpec, InterruptPolicy,
    OutputMode, Profile, StepError, valid_target_triple,
};

pub(super) fn build_workshop(
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
