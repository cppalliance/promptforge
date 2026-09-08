//! Boot planning and one-shot launch coverage.

use std::time::Duration;

use shared_sidecar::ConnectionFile;

use super::{dead_pid, exe_dir, fixture_gateway, live_file, probe_own_image, probe_read_failure};
use crate::gateway::boot::{
    GATEWAY_EXE_NAME, GatewayPlan, no_gateway_error, plan_gateway, sibling_gateway,
    wait_for_launched_file_with,
};

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
    ConnectionFile {
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
        !shared_sidecar::connection_file_path(run.path()).exists(),
        "the stale file was cleaned"
    );
}

#[test]
fn a_stale_file_with_no_sibling_exe_falls_through_to_explicit_config() {
    let run = tempfile::TempDir::new().expect("tempdir");
    ConnectionFile {
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
        !shared_sidecar::connection_file_path(run.path()).exists(),
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
fn the_sibling_probe_finds_only_the_gateway_exe_beside_the_shell() {
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

#[test]
fn the_launch_wait_returns_once_the_file_appears_and_answers() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(fixture_gateway("key"), "key");
    let run_path = run.path().to_owned();
    let written = file.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        written
            .write_to(&run_path)
            .expect("the launched gateway writes");
    });

    let waited = wait_for_launched_file_with(run.path(), Duration::from_secs(5), probe_own_image)
        .expect("the validated file lands and answers");
    assert_eq!(waited, file);
    writer.join().expect("the writer thread ran");
}

#[test]
fn the_launch_wait_times_out_when_no_file_appears() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let error =
        wait_for_launched_file_with(run.path(), Duration::from_millis(150), probe_own_image)
            .expect_err("a gateway that never writes must not hang boot");
    assert!(
        error.to_string().contains("no validated connection file"),
        "the error names the missing file: {error}"
    );
}

#[test]
fn the_launch_wait_rejects_a_key_the_live_process_does_not_accept() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(fixture_gateway("accepted-key"), "rejected-key")
        .write_to(run.path())
        .expect("write");

    let error =
        wait_for_launched_file_with(run.path(), Duration::from_millis(150), probe_own_image)
            .expect_err("an unaccepted connection-file key must not publish");
    assert!(
        error.to_string().contains("no validated connection file"),
        "the error names the validation failure without exposing the key: {error}"
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
