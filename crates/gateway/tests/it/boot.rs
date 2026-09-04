//! Instant-ready boot: the bind is the readiness signal, provisioning runs
//! as the boot `LoadProfile` command on the queue, and quitting while a
//! command is active cancels it and exits promptly.

use std::time::Duration;

use gateway::{ProfileName, ServeOptions};
use serde_json::Value;

use crate::support::{PHASE_TIMEOUT, json_within, send_within};

/// Writes the config and returns its path; the profile selects every model
/// the body declares.
fn write_config(temp: &tempfile::TempDir, body: String) -> std::path::PathBuf {
    let path = temp.path().join("gateway.toml");
    std::fs::write(&path, body).expect("write config");
    path
}

/// Polls `/v1/models` until the catalog is exactly `expected`, so the test
/// observes the boot command's hot-swap without sleeping a fixed delay.
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

/// Reads a streaming response until `marker` appears in the accumulated
/// text, returning what arrived. Bounded by the phase timeout.
async fn read_until(response: &mut reqwest::Response, marker: &str, text: &mut String) {
    while !text.contains(marker) {
        let chunk = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
            .await
            .expect("stream read exceeded the phase timeout")
            .expect("stream read failed");
        let Some(chunk) = chunk else { break };
        text.push_str(std::str::from_utf8(&chunk).expect("SSE frames are UTF-8"));
    }
}

/// The boot command loads the active profile into the initially empty
/// routing table: a remote model appears in `/v1/models` without any
/// provisioning, and the ephemeral CLI override writes no state file.
#[tokio::test]
async fn the_boot_command_loads_the_active_profile_into_an_empty_table() {
    let backend = crate::support::fake_backend().await;
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        format!(
            r#"
config-version = 2

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
models = ["test-model"]
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

    // The boot command runs asynchronously after the bind; poll the catalog
    // until the worker's switch lands the model.
    wait_for_catalog(handle.url(), &http, &["test-model"]).await;
    assert!(
        !temp.path().join("gateway.state.toml").exists(),
        "a command-line profile override stays ephemeral: no state file is written"
    );
    handle.shutdown().expect("graceful shutdown");
}

/// Provisioning is not on the startup path: a config whose local model
/// cannot provision fails the eager `Gateway::from_config` assembly, yet
/// `spawn` binds and serves immediately - the boot command absorbs the
/// failure while the gateway stays reachable with an empty routing table.
#[cfg(feature = "local")]
#[tokio::test]
async fn spawn_leaves_provisioning_to_the_boot_command() {
    let temp = tempfile::tempdir().unwrap();
    let fake_server = temp.path().join("fake-llama-server");
    std::fs::write(&fake_server, b"not a server").expect("write fake server");
    let body = format!(
        r#"
config-version = 2

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
    // call rides a plain thread because the failed store's blocking HTTP
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

/// Quit while a command is active fires the command's cancellation token:
/// the command settles as cancelled and the gateway thread joins promptly
/// instead of waiting out the command's work. The in-flight window here is
/// the switch's bounded drain behind a held request; the mid-download stop
/// is pinned by gateway-local's chunk-boundary test.
#[tokio::test]
async fn quit_during_an_active_command_cancels_it_and_exits_promptly() {
    let (backend, mut arrivals) = crate::support::slow_fake_backend().await;
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "main-model"
description = "the boot profile's model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "other-model"
description = "the switch target's model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[profile]]
name = "main"
models = ["main-model"]

[[profile]]
name = "other"
models = ["other-model"]
"#
        ),
    );
    let options = ServeOptions::new(
        Some(path),
        ProfileName::parse("main").expect("profile name"),
    )
    .with_run_dir(temp.path().join("run"));
    let handle = gateway::spawn(&options).expect("gateway spawns");
    let url = handle.url().to_owned();
    let http = reqwest::Client::new();

    // Wait for the boot command to land main-model in the routing table.
    wait_for_catalog(&url, &http, &["main-model"]).await;

    // Hold a chat request in flight, so the switch command parks in its
    // bounded drain with the request still registered.
    let chat = tokio::spawn({
        let http = http.clone();
        let url = format!("{url}/v1/chat/completions");
        async move {
            http.post(url)
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": "main-model",
                    "messages": [{ "role": "user", "content": "ping" }]
                }))
                .send()
                .await
        }
    });
    let release = crate::support::next_arrival(&mut arrivals).await;

    // The switch goes active and parks in the drain behind the held request.
    let mut switching = send_within(
        http.post(format!("{url}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "other" })),
    )
    .await;
    let mut body = String::new();
    read_until(&mut switching, "loading-profile", &mut body).await;
    assert!(
        body.contains("loading-profile"),
        "the switch command went active: {body}"
    );

    // Quit: the active command's token fires first, then the serve signal.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(handle.shutdown());
    });

    // The command settles as cancelled while the request is still held.
    read_until(&mut switching, "\"status\"", &mut body).await;
    assert!(
        body.contains("cancelled"),
        "the active switch settles as cancelled: {body}"
    );

    // Release the held request so the graceful drain can finish.
    let _ = release.send(());
    let response = chat.await.expect("chat task").expect("chat request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let result = done_rx
        .recv_timeout(PHASE_TIMEOUT)
        .expect("quit during an active command returns promptly");
    result.expect("graceful shutdown");
}
