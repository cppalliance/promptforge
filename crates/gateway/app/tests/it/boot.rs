//! Instant-ready boot: the bind is the readiness signal, the remote table
//! serves from the bind, and local provisioning runs as the boot
//! `LoadProfile` command on the queue when a profile is selected.

use std::time::Duration;

use crate::support::{GatewayProcess, json_within, send_within, wait_for_connection};

mod cli;
mod in_process;
mod lease;
mod ownership;

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
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    )
}

/// A state file naming a profile the config no longer defines degrades the
/// boot to no profile: the remote models serve, the status reports
/// `profile: null`, and the log records the stale-selection warning naming
/// the missing and the defined profiles.
#[test]
fn a_stale_state_file_boots_with_no_profile_and_logs_the_warning() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\n\
         [[model]]\nname = \"test-model\"\ndescription = \"remote\"\ncontext = 1024\n\
         upstream = \"backend-model\"\nendpoints = [\"fake\"]\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    );
    std::fs::write(
        temp.path().join("gateway.state.toml"),
        "active_profile = \"ghost\"\n",
    )
    .expect("write the stale state");
    let run_dir = temp.path().join(".promptforge").join("run");
    let mut gateway = GatewayProcess::spawn_selecting(&path, temp.path(), None);
    let connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    let url = format!("http://127.0.0.1:{}", connection.port);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (status, ids, shutdown) = runtime.block_on(async {
        let http = reqwest::Client::new();
        let status = json_within(
            send_within(
                http.get(format!("{url}/admin/status"))
                    .bearer_auth("test-token"),
            )
            .await,
        )
        .await;
        let ids = crate::support::catalog_ids(
            &http,
            std::net::SocketAddr::from(([127, 0, 0, 1], connection.port)),
        )
        .await;
        let shutdown = send_within(
            http.post(format!("{url}/shutdown"))
                .bearer_auth("test-token"),
        )
        .await
        .status();
        (status, ids, shutdown)
    });
    assert!(
        status["profile"].is_null(),
        "a stale selection boots no profile: {status}"
    );
    assert_eq!(ids, ["test-model"], "the remote models serve");
    assert_eq!(shutdown, reqwest::StatusCode::ACCEPTED);
    drop(runtime);
    let exit = gateway.wait_for_exit(Duration::from_secs(30));
    assert!(exit.success(), "the gateway exits cleanly: {exit}");

    let log = std::fs::read_to_string(
        temp.path()
            .join(".promptforge")
            .join("logs")
            .join("gateway.log"),
    )
    .expect("read the log");
    assert!(
        log.contains(
            "state file selects profile \"ghost\", which is not defined (defined profiles: main); booting with no profile"
        ),
        "the stale-selection warning is logged: {log}"
    );
}

/// A `--profile` naming an undefined profile is an error, not a degraded
/// boot: the process exits with a failure status and the logged chain names
/// the missing profile and the defined ones.
#[test]
fn an_undefined_profile_flag_fails_the_boot_naming_the_defined_profiles() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    );
    let mut gateway = GatewayProcess::spawn_selecting(&path, temp.path(), Some("ghost"));

    let exit = gateway.wait_for_exit(Duration::from_secs(30));
    assert!(!exit.success(), "an undefined --profile fails the boot");
    let log = std::fs::read_to_string(
        temp.path()
            .join(".promptforge")
            .join("logs")
            .join("gateway.log"),
    )
    .expect("the fatal outcome drained to the log file");
    assert!(
        log.contains("active profile ghost is not defined (defined profiles: main)"),
        "the error names the missing profile and the defined ones: {log}"
    );
    assert!(
        !temp
            .path()
            .join(".promptforge")
            .join("run")
            .join("gateway.json")
            .exists(),
        "a failed boot publishes no connection"
    );
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
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
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
        if let Some(file) = gateway_api_discovery::GatewayDiscoveryFile::read(&run_dir)
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

/// The bare invocation is the whole command line: with no `--config` the
/// gateway runs boot discovery, generates the first-run config into the
/// redirected profile, and serves - proved by the gateway discovery file
/// written after the bind. The child is killed once the file lands,
/// before the generated config's boot command can provision anything.
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
        .join("../../../.github/fixtures/workshop-package-smoke");
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
    let file = gateway_api_discovery::GatewayDiscoveryFile::read(&profile.join("run"))
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
