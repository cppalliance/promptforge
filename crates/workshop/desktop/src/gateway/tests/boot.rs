//! Boot planning and one-shot launch coverage.

use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use gateway_api_discovery::{CancellationToken, GatewayDiscoveryFile, HealthError};

use super::{
    dead_pid, exe_dir, fixture_gateway, live_file, owned_candidate, probe_own_image,
    probe_read_failure,
};
#[cfg(windows)]
use crate::gateway::boot::spawn_detached_windows_with;
use crate::gateway::boot::{
    GATEWAY_EXE_NAME, GatewayPlan, no_gateway_error, plan_gateway, sibling_gateway,
};
use crate::gateway::identity::GatewayAttachment;
use crate::gateway::supervisor::wait_for_launched_file_cancellable_with;

const FIXTURE_PHASE_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(windows)]
#[test]
fn windows_detached_spawn_first_attempt_uses_all_required_flags() {
    let mut attempts = Vec::new();

    let child_pid = spawn_detached_windows_with(|flags| {
        attempts.push(flags);
        Ok(41)
    })
    .expect("the first spawn succeeds");

    assert_eq!(child_pid, 41);
    assert_eq!(attempts, [0x0100_0208]);
}

#[cfg(windows)]
#[test]
fn windows_detached_spawn_retries_access_denied_without_breakaway() {
    let mut attempts = Vec::new();

    let child_pid = spawn_detached_windows_with(|flags| {
        attempts.push(flags);
        if attempts.len() == 1 {
            Err(std::io::Error::from_raw_os_error(5))
        } else {
            Ok(42)
        }
    })
    .expect("access denied retries without breakaway");

    assert_eq!(child_pid, 42);
    assert_eq!(attempts, [0x0100_0208, 0x0000_0208]);
}

#[cfg(windows)]
#[test]
fn windows_detached_spawn_does_not_retry_after_success() {
    let mut attempts = 0;

    spawn_detached_windows_with(|_| {
        attempts += 1;
        Ok(43)
    })
    .expect("the first spawn succeeds");

    assert_eq!(attempts, 1);
}

#[cfg(windows)]
#[test]
fn windows_detached_spawn_does_not_retry_other_errors() {
    let mut attempts = 0;

    let error = spawn_detached_windows_with::<u32, _>(|_| {
        attempts += 1;
        Err(std::io::Error::from_raw_os_error(123))
    })
    .expect_err("non-permission errors propagate");

    assert_eq!(error.raw_os_error(), Some(123));
    assert_eq!(attempts, 1);
}

fn workshop_server(
    port: u16,
    api_key: &str,
) -> (tempfile::TempDir, workshop_server_api::ServerHandle) {
    let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
    let server = workshop_server_api::fixtures::spawn(workshop_server_api::Config {
        gateway: workshop_server_api::GatewayConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: api_key.to_owned(),
        },
        server: workshop_server_api::ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            state_dir: state_dir.path().to_owned(),
        },
        agents: workshop_server_api::AgentsConfig::default(),
    })
    .expect("spawn Workshop fixture");
    (state_dir, server)
}

#[test]
fn a_live_file_attaches_without_looking_for_a_sibling_exe() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(fixture_gateway("key"), "key");
    file.write_to(run.path()).expect("write");
    let (_exe, exe_dir) = exe_dir(false);

    match plan_gateway(run.path(), &exe_dir, false, probe_own_image) {
        GatewayPlan::Attach(attached) => assert_eq!(attached, file),
        other => panic!("a live gateway must be attached, not {other:?}"),
    }
}

#[test]
fn no_file_and_a_sibling_exe_launches() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let (_exe, exe_dir) = exe_dir(true);

    match plan_gateway(run.path(), &exe_dir, false, probe_own_image) {
        GatewayPlan::Launch(exe) => assert_eq!(exe, exe_dir.join(GATEWAY_EXE_NAME)),
        other => panic!("a full install with no running gateway must launch, not {other:?}"),
    }
}

#[test]
fn no_file_and_no_sibling_exe_falls_through_to_explicit_config() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let (_exe, exe_dir) = exe_dir(false);

    assert_eq!(
        plan_gateway(run.path(), &exe_dir, true, probe_own_image),
        GatewayPlan::ConfigOnly,
        "a Workshop-only install attaches to the configured LAN gateway"
    );
}

#[test]
fn no_file_no_sibling_exe_and_no_config_fails() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let (_exe, exe_dir) = exe_dir(false);

    assert_eq!(
        plan_gateway(run.path(), &exe_dir, false, probe_own_image),
        GatewayPlan::Fail,
        "nothing to connect to must fail loud, not serve a broken window"
    );
}

#[test]
fn a_stale_file_is_cleaned_and_the_sibling_exe_launches() {
    let run = tempfile::TempDir::new().expect("tempdir");
    GatewayDiscoveryFile {
        pid: dead_pid(),
        ..live_file(1, "k")
    }
    .write_to(run.path())
    .expect("write");
    let (_exe, exe_dir) = exe_dir(true);

    let plan = plan_gateway(run.path(), &exe_dir, false, probe_own_image);
    assert!(
        matches!(plan, GatewayPlan::Launch(_)),
        "a stale file must not block the relaunch: {plan:?}"
    );
    assert!(
        !gateway_api_discovery::gateway_discovery_file_path(run.path()).exists(),
        "the stale file was cleaned"
    );
}

#[test]
fn a_stale_file_with_no_sibling_exe_falls_through_to_explicit_config() {
    let run = tempfile::TempDir::new().expect("tempdir");
    GatewayDiscoveryFile {
        pid: dead_pid(),
        ..live_file(1, "k")
    }
    .write_to(run.path())
    .expect("write");
    let (_exe, exe_dir) = exe_dir(false);

    assert_eq!(
        plan_gateway(run.path(), &exe_dir, true, probe_own_image),
        GatewayPlan::ConfigOnly,
        "a stale file must not wedge the LAN fallback"
    );
    assert!(
        !gateway_api_discovery::gateway_discovery_file_path(run.path()).exists(),
        "the stale file was cleaned"
    );
}

#[test]
fn a_resolve_error_still_launches_the_sibling_exe() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let (_exe, exe_dir) = exe_dir(true);

    let plan = plan_gateway(run.path(), &exe_dir, false, probe_read_failure);
    assert!(
        matches!(plan, GatewayPlan::Launch(_)),
        "a discovery error must not read as no-gateway: {plan:?}"
    );
}

#[test]
fn the_sibling_probe_finds_only_the_gateway_exe_beside_the_desktop_app() {
    let (_dir, with) = exe_dir(true);
    assert_eq!(
        sibling_gateway(&with),
        Some(with.join(GATEWAY_EXE_NAME)),
        "the installed sibling is found"
    );
    let (_dir, without) = exe_dir(false);
    assert_eq!(
        sibling_gateway(&without),
        None,
        "a Workshop-only install has no sibling"
    );
}

/// Runs the launch wait against the test binary's own process image.
fn launch_wait_with<Health>(
    run_dir: &Path,
    budget: Duration,
    health: Health,
) -> anyhow::Result<GatewayDiscoveryFile>
where
    Health: FnMut(&str, Duration, &CancellationToken) -> Result<(), HealthError>,
{
    wait_for_launched_file_cancellable_with(
        run_dir,
        budget,
        &CancellationToken::new(),
        health,
        |run_dir, _| probe_own_image(run_dir),
    )
}

fn launch_wait(run_dir: &Path, budget: Duration) -> anyhow::Result<GatewayDiscoveryFile> {
    launch_wait_with(
        run_dir,
        budget,
        gateway_api_discovery::wait_for_health_cancellable,
    )
}

/// A loopback port nothing listens on.
fn dead_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("reserve a loopback port")
        .port()
}

#[test]
fn the_launch_wait_returns_once_the_file_appears_and_answers() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(fixture_gateway("key"), "key");
    let writer = {
        let run_dir = run.path().to_owned();
        let file = file.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            file.write_to(&run_dir)
                .expect("the launched gateway writes");
        })
    };

    let waited = launch_wait(run.path(), Duration::from_secs(5))
        .expect("the validated file lands and answers");
    writer.join().expect("the writer finishes");
    assert_eq!(waited, file);
}

#[test]
fn the_launch_wait_times_out_when_no_file_appears() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let error = launch_wait(run.path(), Duration::from_millis(150))
        .expect_err("a gateway that never writes must not hang boot");
    assert!(
        error
            .to_string()
            .contains("no validated gateway discovery file"),
        "the error names the missing file: {error}"
    );
}

#[test]
fn the_launch_wait_rejects_a_key_the_live_process_does_not_accept() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(fixture_gateway("accepted-key"), "rejected-key")
        .write_to(run.path())
        .expect("write");

    let error = launch_wait(run.path(), Duration::from_millis(150))
        .expect_err("an unaccepted discovery-file key must not publish");
    let message = format!("{error:#}");
    assert!(
        message.contains("no validated gateway discovery file"),
        "the error names the validation failure: {message}"
    );
    assert!(
        message.contains("bearer was rejected") && !message.contains("rejected-key"),
        "the error reports the rejection without exposing the key: {message}"
    );
}

#[test]
fn the_launch_wait_completes_when_a_dead_port_file_is_replaced_by_a_live_one() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(dead_port(), "key")
        .write_to(run.path())
        .expect("write the dead-port file");
    let live = live_file(fixture_gateway("key"), "key");
    let mut probes = 0;

    let waited = launch_wait_with(
        run.path(),
        Duration::from_secs(5),
        |url, budget, cancellation| {
            probes += 1;
            let probe =
                gateway_api_discovery::wait_for_health_cancellable(url, budget, cancellation);
            if probe.is_err() {
                live.write_to(run.path())
                    .expect("the gateway rewrites its discovery file");
            }
            probe
        },
    )
    .expect("the wait polls past the dead port and attaches to the live gateway");

    assert_eq!(waited, live);
    assert_eq!(
        probes, 2,
        "one failed probe of the dead port, then one of the live gateway"
    );
}

#[test]
fn the_launch_wait_fails_at_its_budget_with_the_last_probe_error() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let port = dead_port();
    live_file(port, "key")
        .write_to(run.path())
        .expect("write the dead-port file");
    let budget = Duration::from_millis(600);
    let mut probes = 0;
    let started = Instant::now();

    let error = launch_wait_with(run.path(), budget, |url, budget, cancellation| {
        probes += 1;
        gateway_api_discovery::wait_for_health_cancellable(url, budget, cancellation)
    })
    .expect_err("a file that never names a live gateway must not hang boot");

    assert!(
        started.elapsed() >= budget,
        "the wait polls until its budget runs out"
    );
    assert!(
        probes >= 2,
        "a failed health probe does not end the wait: {probes} probes"
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("no validated gateway discovery file"),
        "the error names the budget failure: {message}"
    );
    assert!(
        message.contains(&format!("http://127.0.0.1:{port}/health")),
        "the error reports the last probe: {message}"
    );
}

#[test]
fn the_no_gateway_error_names_both_remedies() {
    let message = no_gateway_error().to_string();
    assert!(
        message.contains("promptforge-gateway"),
        "the error names the Gateway component remedy: {message}"
    );
    assert!(
        message.contains("workshop.toml"),
        "the error names the explicit-config remedy: {message}"
    );
}

#[test]
fn matching_boot_publication_survives_closure_and_server_teardown() {
    let mut launched = super::validated_gateway("launched-key");
    let identity = launched.validate("launched-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let attachment = GatewayAttachment::Launched(owned_candidate(identity.clone()));
    let (_state_dir, server) = workshop_server(launched.port(), "launched-key");

    let attachment = attachment.reconcile_publication(Some(identity));
    server.gateway_updater().close_publication();
    drop(attachment);
    let outcome = server.shutdown().expect("server teardown continues");

    assert_eq!(outcome, workshop_server_api::Termination::Graceful);
    assert!(
        !launched.received_shutdown(Duration::from_millis(100)),
        "closure cannot clean up the exact child already published at boot"
    );
}

#[test]
fn closed_boot_publication_cleans_unpublished_owner_then_continues_teardown() {
    let mut launched = super::validated_gateway("launched-key");
    let published = super::validated_gateway("published-key");
    let launched_identity =
        launched.validate("launched-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let published_identity =
        published.validate("published-key", 1_778_000_002, "2026-09-08T18:00:02Z");
    let attachment = GatewayAttachment::Launched(owned_candidate(launched_identity));
    let (_state_dir, server) = workshop_server(published.port(), "published-key");

    server.gateway_updater().close_publication();
    let attachment = attachment.reconcile_publication(Some(published_identity));
    drop(attachment);
    assert!(
        launched.received_shutdown(FIXTURE_PHASE_TIMEOUT),
        "a closed boot cleans only its unpublished authenticated child"
    );
    assert_eq!(
        server.shutdown().expect("server teardown continues"),
        workshop_server_api::Termination::Graceful
    );
}
