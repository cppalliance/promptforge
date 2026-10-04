//! Non-serving invocations: the version and help flags, the `diagnostics`
//! report, and a fatal boot error's logged chain.

use std::time::Duration;

use serde_json::Value;

use super::write_config;

/// `--version` and `--help` exit before logging starts: a pre-existing log
/// is left untouched and never rotated.
#[test]
fn version_and_help_never_rotate_the_log() {
    let temp = tempfile::tempdir().unwrap();
    let logs = temp.path().join(".promptforge").join("logs");
    std::fs::create_dir_all(&logs).expect("create the logs dir");
    std::fs::write(logs.join("gateway.log"), "the running gateway's log").expect("seed the log");
    for flag in ["--version", "--help"] {
        let status = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
            .arg(flag)
            .env("USERPROFILE", temp.path())
            .env("HOME", temp.path())
            .env_remove("RUST_LOG")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("the flag invocation exits");
        assert!(status.success(), "{flag} exits successfully: {status}");
        assert_eq!(
            std::fs::read_to_string(logs.join("gateway.log")).expect("read the log"),
            "the running gateway's log",
            "{flag} left the log untouched"
        );
        assert!(
            !logs.join("gateway.log.1").exists(),
            "{flag} rotated no log"
        );
    }
}

/// `diagnostics` prints the JSON report and exits without serving: a
/// pre-existing log is left untouched and unrotated, no gateway discovery file is
/// created, and the report names the state dir, the config, the logs, and
/// the gateway discovery file with `running: false`.
#[test]
fn diagnostics_reports_without_serving_or_mutating() {
    let temp = tempfile::tempdir().unwrap();
    let logs = temp.path().join(".promptforge").join("logs");
    std::fs::create_dir_all(&logs).expect("create the logs dir");
    std::fs::write(logs.join("gateway.log"), "the running gateway's log").expect("seed the log");
    let config = temp.path().join(".promptforge").join("gateway.toml");
    std::fs::write(&config, "config-version = 0\n").expect("seed the profile config");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("diagnostics")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .output()
        .expect("the diagnostics invocation runs");
    assert!(
        output.status.success(),
        "diagnostics exits successfully: {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let report: Value = serde_json::from_str(&stdout).expect("the report is JSON");
    assert_eq!(
        report["state_dir"].as_str().map(std::path::Path::new),
        Some(temp.path().join(".promptforge").as_path()),
        "the report names the state dir: {stdout}"
    );
    assert_eq!(
        report["config"]["path"].as_str().map(std::path::Path::new),
        Some(config.as_path()),
        "discovery names the profile config: {stdout}"
    );
    assert_eq!(
        report["config"]["exists"], true,
        "the seeded config is reported as existing: {stdout}"
    );
    assert_eq!(report["running"], false, "nothing is running");
    assert_eq!(
        report["logs"]["current"]["exists"], true,
        "the seeded log is reported: {stdout}"
    );
    assert_eq!(
        report["logs"]["retained"].as_array().unwrap().len(),
        5,
        "five retained slots are reported: {stdout}"
    );
    assert_eq!(report["connection_file"]["exists"], false);
    assert!(
        report["version"].as_str().is_some(),
        "the report includes the version"
    );
    assert!(
        !stdout.contains("api_key"),
        "the report omits key material: {stdout}"
    );

    assert_eq!(
        std::fs::read_to_string(logs.join("gateway.log")).expect("read the log"),
        "the running gateway's log",
        "diagnostics left the log untouched"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "diagnostics rotated no log"
    );
    assert!(
        !temp.path().join(".promptforge/run/gateway.json").exists(),
        "diagnostics created no gateway discovery file"
    );
}

/// With a gateway serving, `diagnostics` reports `running: true` - the
/// same already-running detection the handoff path uses - and still never
/// rotates the running gateway's log.
#[test]
fn diagnostics_reports_a_running_gateway_without_rotating_its_log() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    );
    let logs = temp.path().join(".promptforge").join("logs");
    let connection = temp
        .path()
        .join(".promptforge")
        .join("run")
        .join("gateway.json");
    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--config")
        .arg(&path)
        .arg("--profile")
        .arg("main")
        .arg("--no-tray")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the gateway spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !connection.is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "the gateway bound and wrote {}",
            connection.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("diagnostics")
        .arg("--config")
        .arg(&path)
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .output()
        .expect("the diagnostics invocation runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let report: Value = serde_json::from_str(&stdout).expect("the report is JSON");
    assert_eq!(
        report["running"], true,
        "the live gateway is reported as running: {stdout}"
    );
    assert_eq!(report["connection_file"]["exists"], true);
    assert_eq!(
        report["config"]["path"].as_str().map(std::path::Path::new),
        Some(path.as_path()),
        "the explicit --config path is named verbatim: {stdout}"
    );
    assert_eq!(
        report["config"]["exists"], true,
        "the explicit config is reported as existing: {stdout}"
    );

    let _ = first.kill();
    let _ = first.wait();
    assert!(
        !logs.join("gateway.log.1").exists(),
        "diagnostics never rotated the running gateway's log"
    );
    let log = std::fs::read_to_string(logs.join("gateway.log")).expect("read the log");
    assert_eq!(
        log.matches("logging to").count(),
        1,
        "only the serving instance wrote a startup line: {log}"
    );
}

/// A fatal boot failure is logged once with its complete source chain and
/// the queue drains before the process exits with a failure status: the
/// chain lands in the log file, not only on stderr.
#[test]
fn a_fatal_boot_error_lands_in_the_log_with_its_chain() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("no-such-config.toml");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--config")
        .arg(&missing)
        .arg("--no-tray")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("the failing invocation runs");
    assert!(
        !output.status.success(),
        "a missing explicit config fails the boot: {}",
        output.status
    );

    let log = std::fs::read_to_string(
        temp.path()
            .join(".promptforge")
            .join("logs")
            .join("gateway.log"),
    )
    .expect("the fatal outcome drained to the log file");
    assert_eq!(
        log.matches("error:").count(),
        1,
        "the fatal error is logged exactly once: {log}"
    );
    assert!(
        log.contains("caused by:"),
        "the complete source chain is logged: {log}"
    );
    let fatal = log
        .rfind("gateway exiting after a fatal error")
        .expect("the fatal terminal record is logged");
    let final_cause = log
        .rfind("caused by:")
        .expect("the complete source chain is logged");
    assert!(
        fatal > final_cause,
        "the fatal terminal record follows the complete chain: {log}"
    );
    assert!(
        log.lines()
            .last()
            .is_some_and(|line| line.contains("gateway exiting after a fatal error")),
        "the fatal terminal record is last: {log}"
    );
}
