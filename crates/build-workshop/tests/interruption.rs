//! Subprocess coverage for Workshop build interruption and staging cleanup.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const WAIT_BOUND: Duration = Duration::from_secs(15);

#[cfg(windows)]
const TARGET: &str = "x86_64-pc-windows-msvc";
#[cfg(not(windows))]
const TARGET: &str = "x86_64-unknown-linux-gnu";

#[cfg(windows)]
const GATEWAY_NAME: &str = "promptforge-gateway.exe";
#[cfg(not(windows))]
const GATEWAY_NAME: &str = "promptforge-gateway";

#[cfg(windows)]
const SIDECAR_NAME: &str = "promptforge-gateway-x86_64-pc-windows-msvc.exe";
#[cfg(not(windows))]
const SIDECAR_NAME: &str = "promptforge-gateway-x86_64-unknown-linux-gnu";

struct StagingGuard {
    path: PathBuf,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn platform_interrupt_after_staging_kills_child_cleans_and_fails() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf();
    let staged = repository
        .join("crates")
        .join("workshop")
        .join("binaries")
        .join(SIDECAR_NAME);
    assert!(
        !staged.exists(),
        "the interruption test requires absent staging: {}",
        staged.display()
    );
    let _staging_guard = StagingGuard {
        path: staged.clone(),
    };

    let temp = tempfile::tempdir().expect("temporary test root");
    let fake_cargo = write_fake_cargo(&temp);
    let target_root = temp.path().join("target");
    let blocked_marker = temp.path().join("workshop-blocked");
    let stdout_path = temp.path().join("orchestrator.stdout");
    let stderr_path = temp.path().join("orchestrator.stderr");
    let stdout = File::create(&stdout_path).expect("stdout file");
    let stderr = File::create(&stderr_path).expect("stderr file");

    let mut command = Command::new(env!("CARGO_BIN_EXE_build-workshop"));
    command
        .arg("--target")
        .arg(TARGET)
        .env("CARGO", fake_cargo)
        .env("CARGO_TARGET_DIR", &target_root)
        .env("BUILD_WORKSHOP_TEST_TARGET", TARGET)
        .env("BUILD_WORKSHOP_BLOCKED_MARKER", &blocked_marker)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    configure_interruptible_process_group(&mut command);
    let mut orchestrator = command.spawn().expect("start build orchestrator");

    wait_for_path(&blocked_marker, &mut orchestrator, &stderr_path);
    assert!(staged.exists(), "Gateway sidecar was not staged");
    send_platform_interrupt(orchestrator.id(), &temp);

    let status = wait_for_exit(&mut orchestrator, &stderr_path);
    assert!(!status.success(), "interrupted orchestrator succeeded");
    assert!(
        !staged.exists(),
        "interrupted orchestrator left staging at {}",
        staged.display()
    );
    let stderr = fs::read_to_string(stderr_path).expect("orchestrator stderr");
    assert!(
        stderr.contains("Workshop build interrupted"),
        "missing interruption diagnostic: {stderr}"
    );
}

fn wait_for_path(path: &Path, child: &mut Child, stderr_path: &Path) {
    let deadline = Instant::now() + WAIT_BOUND;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => panic!("could not poll orchestrator: {error}"),
        };
        if let Some(status) = status {
            let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
            panic!("orchestrator exited before blocking: {status}\n{stderr}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    terminate_after_timeout(child);
    let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
    panic!("orchestrator did not reach blocked Workshop child\n{stderr}");
}

fn wait_for_exit(child: &mut Child, stderr_path: &Path) -> ExitStatus {
    let deadline = Instant::now() + WAIT_BOUND;
    while Instant::now() < deadline {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => panic!("could not poll orchestrator: {error}"),
        };
        if let Some(status) = status {
            return status;
        }
        thread::sleep(Duration::from_millis(20));
    }
    terminate_after_timeout(child);
    let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
    panic!("orchestrator did not exit after interruption\n{stderr}");
}

fn terminate_after_timeout(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn write_fake_cargo(temp: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = temp.path().join("fake-cargo");
    if let Err(error) = fs::write(
        &path,
        format!(
            r#"#!/bin/sh
if [ "$1" = "build" ] && [ "$3" = "gateway" ]; then
  mkdir -p "$CARGO_TARGET_DIR/$BUILD_WORKSHOP_TEST_TARGET/debug"
  printf gateway > "$CARGO_TARGET_DIR/$BUILD_WORKSHOP_TEST_TARGET/debug/{GATEWAY_NAME}"
  exit 0
fi
if [ "$1" = "build" ] && [ "$3" = "workshop" ]; then
  printf blocked > "$BUILD_WORKSHOP_BLOCKED_MARKER"
  while :; do sleep 1; done
fi
exit 91
"#
        ),
    ) {
        panic!("could not write fake Cargo: {error}");
    }
    if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o755)) {
        panic!("could not make fake Cargo executable: {error}");
    }
    path
}

#[cfg(windows)]
fn write_fake_cargo(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("fake-cargo.cmd");
    if let Err(error) = fs::write(
        &path,
        format!(
            r#"@echo off
if "%1"=="build" if "%3"=="gateway" goto gateway
if "%1"=="build" if "%3"=="workshop" goto workshop
exit /b 91
:gateway
if not exist "%CARGO_TARGET_DIR%\%BUILD_WORKSHOP_TEST_TARGET%\debug" mkdir "%CARGO_TARGET_DIR%\%BUILD_WORKSHOP_TEST_TARGET%\debug"
> "%CARGO_TARGET_DIR%\%BUILD_WORKSHOP_TEST_TARGET%\debug\{GATEWAY_NAME}" echo gateway
exit /b 0
:workshop
> "%BUILD_WORKSHOP_BLOCKED_MARKER%" echo blocked
:blocked
ping -n 2 127.0.0.1 >nul
goto blocked
"#
        ),
    ) {
        panic!("could not write fake Cargo: {error}");
    }
    path
}

#[cfg(unix)]
fn configure_interruptible_process_group(_command: &mut Command) {}

#[cfg(windows)]
fn configure_interruptible_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn send_platform_interrupt(process_id: u32, _temp: &TempDir) {
    let status = Command::new("kill")
        .arg("-INT")
        .arg(process_id.to_string())
        .status();
    let status = match status {
        Ok(status) => status,
        Err(error) => panic!("could not send SIGINT: {error}"),
    };
    assert!(status.success(), "kill could not send SIGINT");
}

#[cfg(windows)]
fn send_platform_interrupt(process_id: u32, temp: &TempDir) {
    let sender = temp.path().join("send-interrupt.ps1");
    if let Err(error) = fs::write(
        &sender,
        r#"Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public static class ConsoleSignal {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GenerateConsoleCtrlEvent(uint signal, uint processGroupId);
}
"@
if (-not [ConsoleSignal]::GenerateConsoleCtrlEvent(1, [uint32]$args[0])) {
    exit 1
}
"#,
    ) {
        panic!("could not write interrupt sender: {error}");
    }
    let status = Command::new("powershell.exe")
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(sender)
        .arg(process_id.to_string())
        .status();
    let status = match status {
        Ok(status) => status,
        Err(error) => panic!("could not send console interrupt: {error}"),
    };
    assert!(status.success(), "could not send console interrupt");
}
