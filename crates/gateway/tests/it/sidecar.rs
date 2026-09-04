//! The connection-file lifecycle: [`spawn`] writes `gateway.json` after
//! the bind and [`GatewayHandle::shutdown`] removes it.

use std::time::Duration;

use gateway::{ProfileName, ServeOptions, spawn};

/// A minimal boot config: one unreachable fake backend, one profile. The
/// backend is never contacted at boot.
const CATALOG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = ["alpha-model"]
"#;

#[test]
fn spawn_writes_the_connection_file_and_shutdown_removes_it() {
    let temp = tempfile::TempDir::new().unwrap();
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, CATALOG).unwrap();
    let run_dir = temp.path().join("run");
    let options = ServeOptions::new(
        config_path,
        ProfileName::parse("alpha").expect("profile name"),
    )
    .with_run_dir(run_dir.clone());

    let gateway = spawn(&options).expect("the gateway boots");

    // The file exists the moment spawn returns, carrying the real bind.
    let file = shared_sidecar::ConnectionFile::read(&run_dir)
        .expect("the connection file reads")
        .expect("the connection file exists after the bind");
    assert_eq!(file.pid, std::process::id());
    let port: u16 = gateway
        .url()
        .strip_prefix("http://127.0.0.1:")
        .expect("the gateway bound loopback")
        .parse()
        .expect("the url carries a port");
    assert_eq!(file.port, port, "the file carries the bound port");
    assert_eq!(file.api_key, "test-token");
    assert_eq!(file.version, env!("CARGO_PKG_VERSION"));
    assert!(file.epoch > 0);
    // The file's parameters reach the live gateway.
    shared_sidecar::wait_for_health(gateway.url(), Duration::from_secs(5))
        .expect("the health endpoint answers through the file's port");

    gateway.shutdown().expect("clean shutdown");
    assert_eq!(
        shared_sidecar::ConnectionFile::read(&run_dir).expect("read after shutdown"),
        None,
        "a clean shutdown removes the connection file"
    );
}
