//! Instant-ready boot: the bind is the readiness signal, provisioning runs
//! as the boot `LoadProfile` command on the queue, and quitting while a
//! command is active cancels it and exits promptly.

use std::time::Duration;

use gateway::{ProfileName, ServeOptions};
use serde_json::Value;

use crate::support::{GatewayProcess, PHASE_TIMEOUT, json_within, send_within};

/// Writes the config and returns its path; the profile selects every model
/// the body declares.
fn write_config(temp: &tempfile::TempDir, body: String) -> std::path::PathBuf {
    let path = temp.path().join("gateway.toml");
    std::fs::write(&path, body).expect("write config");
    path
}

fn race_config(temp: &tempfile::TempDir) -> std::path::PathBuf {
    write_config(
        temp,
        "config-version = 2\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    )
}

#[cfg(feature = "test-fixtures")]
fn spawn_at_ownership_rendezvous(
    config: &std::path::Path,
    home: &std::path::Path,
) -> (GatewayProcess, GatewayProcess) {
    let first_ready = home.join("first-before-ownership.ready");
    let second_ready = home.join("second-before-ownership.ready");
    let release = home.join("release-ownership-race");
    let mut first = GatewayProcess::spawn_gated(config, home, &first_ready, &release);
    let mut second = GatewayProcess::spawn_gated(config, home, &second_ready, &release);
    let deadline = std::time::Instant::now() + PHASE_TIMEOUT;
    while !(first_ready.is_file() && second_ready.is_file()) {
        assert!(
            first.try_wait().is_none() && second.try_wait().is_none(),
            "both Gateway processes remain blocked at the ownership rendezvous"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "both Gateway processes reached the ownership rendezvous"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !shared_sidecar::gateway_discovery_file_path(&home.join(".promptforge").join("run"))
            .exists(),
        "neither process can acquire ownership or publish before release"
    );
    std::fs::write(&release, b"release").expect("release both ownership contenders");
    (first, second)
}

fn wait_for_connection(
    run_dir: &std::path::Path,
    timeout: Duration,
) -> shared_sidecar::GatewayDiscoveryFile {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(connection) = shared_sidecar::GatewayDiscoveryFile::read(run_dir)
            .expect("read the gateway discovery file")
        {
            return connection;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a Gateway owner did not publish within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "test-fixtures")]
fn assert_exactly_one_process_owns(
    first: &mut GatewayProcess,
    second: &mut GatewayProcess,
    connection: &shared_sidecar::GatewayDiscoveryFile,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let first_status = first.try_wait();
        let second_status = second.try_wait();
        match (first_status, second_status) {
            (Some(status), None) => {
                assert!(status.success(), "the first launch hands off: {status}");
                assert_eq!(
                    connection.pid,
                    second.id(),
                    "the published connection names the surviving owner"
                );
                let output = first.stdout();
                assert!(
                    output.contains(&format!("http://127.0.0.1:{}/auth?key=", connection.port)),
                    "the first launch printed the owner's handoff URL: {output}"
                );
                return false;
            }
            (None, Some(status)) => {
                assert!(status.success(), "the second launch hands off: {status}");
                assert_eq!(
                    connection.pid,
                    first.id(),
                    "the published connection names the surviving owner"
                );
                let output = second.stdout();
                assert!(
                    output.contains(&format!("http://127.0.0.1:{}/auth?key=", connection.port)),
                    "the second launch printed the owner's handoff URL: {output}"
                );
                return true;
            }
            (Some(first_status), Some(second_status)) => {
                panic!(
                    "both Gateway launches exited instead of leaving one owner: \
                     {first_status}, {second_status}"
                );
            }
            (None, None) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "both Gateway launches kept serving instead of electing one owner"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "test-fixtures")]
fn assert_one_canonical_log(home: &std::path::Path) {
    let logs = home.join(".promptforge").join("logs");
    let log_path = logs.join("gateway.log");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let log = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("logging to") {
            break log;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the owner did not write its canonical startup log"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        !logs.join("gateway.log.1").exists(),
        "a losing process never rotates the owner's canonical log"
    );
    assert_eq!(
        log.matches("promptforge-gateway").count(),
        1,
        "only the owner writes the versioned startup record: {log}"
    );
    assert_eq!(
        log.matches("logging to").count(),
        1,
        "only the owner initializes canonical logging: {log}"
    );
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

/// A headless invocation with `--config` bookends its serving log: the
/// versioned launch record is first, and route-driven shutdown leaves the
/// clean terminal record last. The real binary is spawned with the profile
/// directory redirected into a temp dir (via the home variables `home_dir`
/// reads), so the run touches nothing outside it.
#[test]
fn headless_serve_bookends_the_log_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 2\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    );
    let run_dir = temp.path().join(".promptforge").join("run");
    let log = temp
        .path()
        .join(".promptforge")
        .join("logs")
        .join("gateway.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
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
        .expect("the gateway binary spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let connection = loop {
        if let Some(file) = shared_sidecar::GatewayDiscoveryFile::read(&run_dir)
            .expect("read the gateway discovery file")
        {
            break file;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the gateway bound and wrote {}",
            run_dir.join("gateway.json").display()
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = runtime
        .block_on(async {
            reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/shutdown", connection.port))
                .bearer_auth(&connection.api_key)
                .send()
                .await
        })
        .expect("the shutdown POST answers");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    drop(runtime);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the gateway process") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the route-driven shutdown did not stop the gateway");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the gateway exits cleanly: {status}");

    let contents = std::fs::read_to_string(&log).expect("read the drained log file");
    let lines = contents.lines().collect::<Vec<_>>();
    assert!(
        contents.contains("gateway.log"),
        "the startup line names the log path: {contents}"
    );
    assert!(
        lines.first().is_some_and(|line| line.contains(&format!(
            "promptforge-gateway {} starting",
            env!("CARGO_PKG_VERSION")
        ))),
        "the versioned launch record is first: {contents}"
    );
    assert!(
        lines
            .last()
            .is_some_and(|line| line.contains("gateway exiting")),
        "the clean terminal record is last: {contents}"
    );
}

/// The bare invocation needs no subcommand: with no `--config` the gateway
/// runs boot discovery, generates the first-run config into the redirected
/// profile, and serves - proved by the gateway discovery file written after the
/// bind. The child is killed once the file lands, before the generated
/// config's boot command can provision anything.
#[test]
fn the_root_invocation_serves_with_boot_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let connection = temp
        .path()
        .join(".promptforge")
        .join("run")
        .join("gateway.json");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--no-tray")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the gateway binary spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !connection.is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "the discovered boot bound and wrote {}",
            connection.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        temp.path()
            .join(".promptforge")
            .join("gateway.toml")
            .is_file(),
        "first-run generation wrote the profile config"
    );
}

/// The clean-machine package fixture must be a complete existing profile:
/// Workshop launches the sibling Gateway without profile arguments, so the
/// fixture itself has to select an empty profile before readiness can publish.
#[test]
fn the_workshop_package_smoke_profile_boots_without_cli_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join(".promptforge");
    std::fs::create_dir(&profile).expect("create isolated profile");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/fixtures/workshop-package-smoke");
    for entry in std::fs::read_dir(&fixture).expect("read package smoke fixture") {
        let entry = entry.expect("read fixture entry");
        std::fs::copy(entry.path(), profile.join(entry.file_name())).expect("copy fixture entry");
    }
    let connection = profile.join("run").join("gateway.json");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--no-tray")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("PROMPTFORGE_PROFILE")
        .env_remove("PROMPTFORGE_GATEWAY_CONFIG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the packaged Gateway binary spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !connection.is_file() {
        assert!(
            child.try_wait().expect("poll packaged Gateway").is_none(),
            "the package smoke profile keeps the bare Gateway serving"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the package smoke profile publishes a gateway discovery file"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let file = shared_sidecar::GatewayDiscoveryFile::read(&profile.join("run"))
        .expect("read package connection")
        .expect("package connection exists");
    assert_eq!(
        file.pid,
        child.id(),
        "the launched Gateway owns publication"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// A second launch hands off to the running gateway and exits: under
/// `--print-url` it prints the running gateway's own Settings URL. Because
/// the handoff runs before logging starts, the running gateway's log is
/// never rotated and gains no second startup line.
#[test]
fn a_second_instance_hands_off_without_rotating_the_log() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 2\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
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
        .expect("the first gateway spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !connection.is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "the first gateway bound and wrote {}",
            connection.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(&connection).expect("read the gateway discovery file"),
    )
    .expect("the gateway discovery file is JSON");
    let port = file["port"].as_u64().expect("the file carries a port");

    // The second launch: the handoff prints the running gateway's URL and
    // exits. A regression to a normal boot would serve instead, so the
    // exit wait is bounded and the kill is the failure path.
    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--config")
        .arg(&path)
        .arg("--profile")
        .arg("main")
        .arg("--print-url")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the second gateway spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = second.try_wait().expect("poll the second instance") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = second.kill();
            panic!("the second instance booted a duplicate server instead of handing off");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the handoff exits successfully: {status}");
    let mut stdout = String::new();
    std::io::Read::read_to_string(
        &mut second.stdout.take().expect("piped stdout"),
        &mut stdout,
    )
    .expect("read the second instance's stdout");
    assert!(
        stdout.contains(&format!("http://127.0.0.1:{port}/auth?key=")),
        "the printed URL is the running gateway's own handoff URL: {stdout}"
    );

    let _ = first.kill();
    let _ = first.wait();
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the handoff never rotated the running gateway's log"
    );
    let log = std::fs::read_to_string(logs.join("gateway.log")).expect("read the log");
    assert_eq!(
        log.matches("logging to").count(),
        1,
        "only the serving instance wrote a startup line: {log}"
    );
}

/// Two direct process launches against an absent record elect one process
/// owner before either can initialize logging or bind.
#[cfg(feature = "test-fixtures")]
#[test]
fn simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let (mut first, mut second) = spawn_at_ownership_rendezvous(&config, temp.path());

    let connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    let first_owns = assert_exactly_one_process_owns(&mut first, &mut second, &connection);
    assert_one_canonical_log(temp.path());
    assert!(
        shared_sidecar::GatewayInstanceLease::try_acquire(&run_dir)
            .expect("contend for the live Gateway's process lease")
            .is_none(),
        "the surviving process keeps its lease for its serving lifetime"
    );

    if first_owns {
        first.stop(Duration::from_secs(5));
    } else {
        second.stop(Duration::from_secs(5));
    }
}

/// A Workshop-elected launch keeps the parent-side `LaunchLock` while two
/// real Gateway processes race. Gateway ownership must not contend on that
/// parent lock, and only one child may survive.
#[cfg(feature = "test-fixtures")]
#[test]
fn workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let shared_sidecar::LaunchDecision::Launch(workshop_lock) =
        shared_sidecar::launch_or_attach(&run_dir, Duration::from_secs(5))
            .expect("Workshop wins the parent launch election")
    else {
        panic!("an empty run directory elects the Workshop launcher");
    };

    let (mut workshop_launch, mut direct_launch) =
        spawn_at_ownership_rendezvous(&config, temp.path());
    let connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    let workshop_owns =
        assert_exactly_one_process_owns(&mut workshop_launch, &mut direct_launch, &connection);
    assert_one_canonical_log(temp.path());
    drop(workshop_lock);

    if workshop_owns {
        workshop_launch.stop(Duration::from_secs(5));
    } else {
        direct_launch.stop(Duration::from_secs(5));
    }
}

/// The ordinary production binary has no compiled rendezvous hook: even
/// environment names used by the feature-enabled fixture are inert.
#[cfg(not(feature = "test-fixtures"))]
#[test]
fn the_default_binary_ignores_test_rendezvous_environment() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let ready = temp.path().join("default-build-ready");
    let absent_release = temp.path().join("default-build-release");
    let mut gateway =
        GatewayProcess::spawn_with_inert_rendezvous(&config, temp.path(), &ready, &absent_release);

    let connection = wait_for_connection(&run_dir, PHASE_TIMEOUT);
    assert_eq!(
        connection.pid,
        gateway.id(),
        "the default binary serves without consulting test rendezvous state"
    );
    assert!(
        !ready.exists(),
        "the default binary never writes the test synchronization marker"
    );
    gateway.stop(Duration::from_secs(5));
}

/// A process that loses the lifetime lease to a silent owner exits nonzero
/// after the bounded publication wait. The failure stays on stderr and never
/// initializes or rotates canonical logging.
#[test]
fn a_silent_process_lease_owner_causes_a_bounded_console_only_failure() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the owner's canonical log");
    let _owner = shared_sidecar::GatewayInstanceLease::try_acquire(&run_dir)
        .expect("acquire the silent owner lease")
        .expect("the test owns the process lease");
    let connection_path = shared_sidecar::gateway_discovery_file_path(&run_dir);
    std::fs::write(&connection_path, b"owner-is-still-publishing")
        .expect("seed an unreadable owner record");

    let mut loser = GatewayProcess::spawn(&config, temp.path());
    let status = loser.wait_for_exit(Duration::from_secs(15));
    assert!(
        !status.success(),
        "a lease loser without a validated owner record exits nonzero"
    );
    let stderr = loser.stderr();
    assert!(
        stderr.contains("process owner published no validated connection"),
        "the console error names the bounded ownership failure: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded canonical log"),
        "owner-log-sentinel",
        "the losing process never initializes canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the losing process never rotates the canonical log"
    );
    assert_eq!(
        std::fs::read(&connection_path).expect("read the seeded owner record"),
        b"owner-is-still-publishing",
        "the losing process never cleans or rewrites shared connection state"
    );
}

/// A lease holder that cannot read existing shared state exits through the
/// console-only startup path, preserving the source chain and canonical log.
#[test]
fn a_connection_resolution_failure_is_console_only() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the canonical log");
    let connection_path = shared_sidecar::gateway_discovery_file_path(&run_dir);
    std::fs::create_dir_all(&connection_path).expect("create unreadable connection fixture");

    let mut gateway = GatewayProcess::spawn(&config, temp.path());
    let status = gateway.wait_for_exit(Duration::from_secs(5));

    assert!(
        !status.success(),
        "an unresolved connection record prevents boot"
    );
    let stderr = gateway.stderr();
    assert!(
        stderr.contains("resolve existing Gateway connection before startup")
            && stderr.contains("caused by: read"),
        "stderr retains the resolution failure and source chain: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded log"),
        "owner-log-sentinel",
        "the failure never initializes or rotates canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the failure creates no retained log"
    );
    assert!(
        connection_path.is_dir(),
        "the failure leaves uncertain connection state untouched"
    );
}

/// Terminating the real owner leaves its connection record behind but
/// releases the operating-system lease, so a later direct launch cleans the
/// stale record and becomes the sole owner.
#[test]
fn a_direct_launch_recovers_the_lease_from_a_terminated_owner() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let mut first = GatewayProcess::spawn(&config, temp.path());
    let first_connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    assert_eq!(first_connection.pid, first.id());
    assert!(
        shared_sidecar::GatewayInstanceLease::try_acquire(&run_dir)
            .expect("contend for the first owner's process lease")
            .is_none(),
        "the first process owns the lifetime lease"
    );
    first.stop(Duration::from_secs(5));

    let mut replacement = GatewayProcess::spawn(&config, temp.path());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let replacement_connection = loop {
        if let Some(connection) = shared_sidecar::GatewayDiscoveryFile::read(&run_dir)
            .expect("read replacement connection")
            && connection.pid == replacement.id()
        {
            break connection;
        }
        assert!(
            replacement.try_wait().is_none(),
            "the replacement exited before taking the dead owner's lease"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the replacement did not publish after the owner was terminated"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_ne!(
        replacement_connection.pid, first_connection.pid,
        "the stale owner record was replaced"
    );
    replacement.stop(Duration::from_secs(5));
}

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
    std::fs::write(&config, "config-version = 2\n").expect("seed the profile config");

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
        "the report carries the version"
    );
    assert!(
        !stdout.contains("api_key"),
        "the report carries no key material: {stdout}"
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
        "config-version = 2\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
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

/// A config with two profiles over one backend, so a switch from `main` to
/// `other` exercises the switch machinery against the slow backend.
fn two_profile_config(backend: std::net::SocketAddr) -> String {
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
    )
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
    let path = write_config(&temp, two_profile_config(backend));
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

/// A save never waits for a running switch: the `LoadProfile` command does
/// not hold the apply lock, so `PUT /admin/config` completes within the
/// phase timeout while the switch is parked in its drain behind a held
/// request, and the switch still finishes once the request is released.
#[tokio::test]
async fn a_save_completes_while_a_switch_command_is_parked() {
    let (backend, mut arrivals) = crate::support::slow_fake_backend().await;
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(&temp, two_profile_config(backend));
    let options = ServeOptions::new(
        Some(path.clone()),
        ProfileName::parse("main").expect("profile name"),
    )
    .with_run_dir(temp.path().join("run"));
    let handle = gateway::spawn(&options).expect("gateway spawns");
    let url = handle.url().to_owned();
    let http = reqwest::Client::new();
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
    let mut switching = send_within(
        http.post(format!("{url}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "other" })),
    )
    .await;
    let mut body = String::new();
    read_until(&mut switching, "loading-profile", &mut body).await;

    // The save lands while the switch is parked; `send_within` bounds it by
    // the phase timeout, well inside the drain's own deadline.
    let document = json_within(
        send_within(
            http.get(format!("{url}/admin/config"))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    let save = send_within(
        http.put(format!("{url}/admin/config"))
            .bearer_auth("test-token")
            .json(&document),
    )
    .await;
    assert_eq!(
        save.status(),
        reqwest::StatusCode::OK,
        "the save does not wait for the parked switch"
    );
    assert!(
        path.with_file_name("gateway.toml.next").is_file(),
        "the save staged its shadow while the switch was active"
    );

    // Release the held request: the switch drains and completes.
    let _ = release.send(());
    let response = chat.await.expect("chat task").expect("chat request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    read_until(&mut switching, "\"status\"", &mut body).await;
    assert!(
        body.contains("\"ready\""),
        "the parked switch completes after the release: {body}"
    );
    handle.shutdown().expect("graceful shutdown");
}
