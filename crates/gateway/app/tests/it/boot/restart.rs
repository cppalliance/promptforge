//! Test restart and rollback, against the production binary: a
//! started gateway removes the `.prev` copies Apply kept, and a supervised
//! gateway asked to restart exits with `RESTART_REQUESTED`.

use std::process::{Command, Stdio};
use std::time::Duration;

use super::write_config;
use crate::support::{GatewayProcess, wait_for_connection};

const CONFIG: &str = "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\n\
                      api_key = \"test-token\"\n\n[[profile]]\nname = \"main\"\nmodels = []\n";

#[test]
fn a_started_gateway_removes_the_rollback_copies() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = write_config(&temp, CONFIG.to_string());
    let config_prev = temp.path().join("gateway.toml.prev");
    let env_prev = temp.path().join("gateway.env.prev");

    std::fs::write(&config_prev, CONFIG).expect("writing a config rollback file should succeed");
    std::fs::write(&env_prev, "").expect("writing an env rollback file should succeed");

    let mut gateway = GatewayProcess::spawn(&config_path, temp.path());
    wait_for_connection(
        &temp.path().join(".runvil").join("run"),
        Duration::from_secs(30),
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while config_prev.exists() || env_prev.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the started gateway should remove its rollback copies"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    gateway.stop(Duration::from_secs(10));
}

#[tokio::test]
async fn a_supervised_gateway_exits_with_restart_requested() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = write_config(&temp, CONFIG.to_string());

    // stdin is a held-open pipe, as the supervisor gives it; a closed stdin
    // would trip the watchdog and end the gateway with a stop instead.
    let mut child = Command::new(env!("CARGO_BIN_EXE_runvil-gateway"))
        .arg("--config")
        .arg(&config_path)
        .arg("--profile")
        .arg("main")
        .arg("--no-tray")
        .arg("--supervised")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the supervised gateway should spawn");

    let connection = wait_for_connection(
        &temp.path().join(".runvil").join("run"),
        Duration::from_secs(30),
    );

    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/admin/restart",
            connection.port
        ))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the restart request should send");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .expect("polling the gateway should succeed")
        {
            break status;
        }

        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the gateway did not exit after the restart request");
        }

        std::thread::sleep(Duration::from_millis(50));
    };

    assert_eq!(
        status.code(),
        Some(i32::from(gateway::RESTART_REQUESTED)),
        "a requested restart should exit with RESTART_REQUESTED (75), got: {status}"
    );
}
