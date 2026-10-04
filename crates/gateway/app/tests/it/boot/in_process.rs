//! In-process boots through `gateway::spawn`: the remote table serves from the
//! bind, provisioning runs as the boot command, and a profileless boot 404s
//! local models.

use std::time::Duration;

use gateway::{ProfileName, ServeOptions};
use serde_json::Value;

use super::write_config;
use crate::support::{json_within, send_within};

/// Polls `/v1/models` until the catalog is exactly `expected`, so the test
/// observes the published table without sleeping a fixed delay.
async fn wait_for_catalog(url: &str, http: &reqwest::Client, expected: &[&str]) {
    let mut ids = Vec::new();
    for _ in 0..100 {
        let catalog = json_within(
            send_within(
                http.get(format!("{url}/v1/models"))
                    .bearer_auth("test-token"),
            )
            .await,
        )
        .await;
        ids = catalog["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>();
        if ids == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ids, expected, "the boot command hot-swaps the catalog");
}

/// The remote table is published at the bind: a remote model appears in
/// `/v1/models` without any provisioning, and the ephemeral CLI override
/// writes no state file.
#[tokio::test]
async fn the_remote_table_serves_from_the_bind_under_a_cli_profile() {
    let backend = crate::support::fake_backend().await;
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        format!(
            r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[profile]]
name = "main"
models = []
"#
        ),
    );
    let options = ServeOptions::new(
        Some(path),
        ProfileName::parse("main").expect("profile name"),
    )
    .with_run_dir(temp.path().join("run"));
    let handle = gateway::spawn(&options).expect("gateway spawns");
    let http = reqwest::Client::new();

    // The remote table is published before the bind; the poll only absorbs
    // the readiness handshake.
    wait_for_catalog(handle.url(), &http, &["test-model"]).await;
    assert!(
        !temp.path().join("gateway.state.toml").exists(),
        "a command-line profile override stays ephemeral: no state file is written"
    );
    handle.shutdown().expect("graceful shutdown");
}

/// Provisioning happens off the startup path: a config whose local model
/// cannot provision fails the eager `Gateway::from_config` assembly, yet
/// `spawn` binds and serves immediately - the boot command absorbs the
/// failure while the gateway stays reachable with its (here empty) remote
/// routing table.
#[cfg(feature = "local")]
#[tokio::test]
async fn spawn_leaves_provisioning_to_the_boot_command() {
    let temp = tempfile::tempdir().unwrap();
    let fake_server = temp.path().join("fake-llama-server");
    std::fs::write(&fake_server, b"not a server").expect("write fake server");
    let body = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache}'
llama_server_path = '{server}'

[[local_model]]
name = "missing-model"
description = "a model whose source file is absent"
source = "{missing}"
context = 4096

[[profile]]
name = "main"
models = ["missing-model"]
"#,
        cache = temp
            .path()
            .join("cache")
            .display()
            .to_string()
            .replace('\\', "/"),
        server = fake_server.display().to_string().replace('\\', "/"),
        missing = temp
            .path()
            .join("absent.gguf")
            .display()
            .to_string()
            .replace('\\', "/"),
    );

    // The eager assembly provisions inline, so the absent source fails it:
    // the failure is what proves provisioning runs on this path at all. The
    // call runs on a plain thread because the failed store's blocking HTTP
    // client cannot drop inside the test's async context.
    let eager = body.clone();
    let text = std::thread::spawn(move || {
        let config = gateway::Config::from_toml_str(&eager).expect("config parses");
        let error = gateway::Gateway::from_config(&config, gateway::ProfilesContext::default())
            .expect_err("eager assembly provisions and fails on the absent source");
        let mut text = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            text.push_str("; ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        text
    })
    .join()
    .expect("the eager assembly thread");
    assert!(
        text.contains("not an existing file"),
        "the eager failure is the model's provisioning: {text}"
    );

    // The spawn path binds and serves with the provisioning failure still
    // ahead of it, queued as the boot command.
    let path = write_config(&temp, body);
    let options = ServeOptions::new(
        Some(path),
        ProfileName::parse("main").expect("profile name"),
    )
    .with_run_dir(temp.path().join("run"));
    let handle = gateway::spawn(&options).expect("spawn binds without provisioning");

    let health = send_within(reqwest::Client::new().get(format!("{}/health", handle.url()))).await;
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let catalog = json_within(
        send_within(
            reqwest::Client::new()
                .get(format!("{}/v1/models", handle.url()))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(
        catalog["data"].as_array().unwrap().len(),
        0,
        "the routing table starts empty; the boot command's failure stays in the queue: {catalog}"
    );
    handle.shutdown().expect("graceful shutdown");
}

/// A config declaring no `[[profile]]` and no state file boots with no
/// profile: every remote model routes from the bind, a `[[local_model]]`
/// the catalog declares but nothing selects is a plain 404 (no boot command
/// runs, so nothing promises it), the status reports `profile: null`, and
/// no state file appears.
#[tokio::test]
async fn a_boot_with_no_profile_serves_remote_models_and_404s_local_ones() {
    let backend = crate::support::fake_backend().await;
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        format!(
            r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[local_model]]
name = "local-model"
description = "declared but selected by no profile"
source = "/models/local.gguf"
context = 4096
"#
        ),
    );
    let options =
        ServeOptions::new(Some(path), None::<ProfileName>).with_run_dir(temp.path().join("run"));
    let handle = gateway::spawn(&options).expect("gateway spawns");
    let http = reqwest::Client::new();

    let catalog = json_within(
        send_within(
            http.get(format!("{}/v1/models", handle.url()))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    let ids = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(ids, ["test-model"], "the remote table serves from the bind");
    let status = json_within(
        send_within(
            http.get(format!("{}/admin/status", handle.url()))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert!(status["profile"].is_null(), "no profile runs: {status}");
    assert_eq!(
        status["queue"]["active"],
        Value::Null,
        "no boot command runs with no profile: {status}"
    );
    let local = send_within(
        http.post(format!("{}/v1/chat/completions", handle.url()))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "local-model",
                "messages": [{"role": "user", "content": "ping"}]
            })),
    )
    .await;
    assert_eq!(
        local.status(),
        reqwest::StatusCode::NOT_FOUND,
        "an unselected local model is a plain 404"
    );
    let remote = send_within(
        http.post(format!("{}/v1/chat/completions", handle.url()))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "ping"}]
            })),
    )
    .await;
    assert_eq!(remote.status(), reqwest::StatusCode::OK);
    assert!(
        !temp.path().join("gateway.state.toml").exists(),
        "booting with no profile writes no state file"
    );
    handle.shutdown().expect("graceful shutdown");
}
