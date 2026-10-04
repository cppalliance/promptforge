//! Unit tests for failure, interruption, and sidecar cleanup paths.

use super::*;

#[test]
fn gateway_failure_still_removes_any_staged_sidecar() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let triple = "x86_64-pc-windows-msvc";
    let mut runner = FakeRunner::with_responses(vec![failure("gateway broke"), success("")]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some(triple.to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("Gateway failure");

    assert!(error.primary.contains("Gateway build failed"), "{error}");
    assert!(error.primary.contains("gateway broke"), "{error}");
    assert_eq!(error.cleanup, None);
    assert_eq!(runner.commands.len(), 2);
    assert_eq!(
        runner.commands[1],
        command(
            environment,
            "selected-node",
            &[
                environment
                    .workspace_root
                    .join("tools")
                    .join("stage-gateway-sidecar.mjs")
                    .to_str()
                    .expect("UTF-8 script"),
                "remove",
                "--target",
                triple,
            ],
            OutputMode::Inherit,
        )
    );
}

#[test]
fn workshop_failure_is_primary_when_cleanup_also_fails() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let triple = "x86_64-unknown-linux-gnu";
    let mut runner = FakeRunner::with_responses(vec![
        success(""),
        success(""),
        failure("workshop broke"),
        FakeResponse::SpawnFailed(io::ErrorKind::NotFound, "node disappeared"),
    ]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Release,
            target: Some(triple.to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("Workshop and cleanup failure");

    assert!(error.primary.contains("Workshop build failed"), "{error}");
    assert!(error.primary.contains("workshop broke"), "{error}");
    let cleanup = error.cleanup.expect("separate cleanup failure");
    assert!(cleanup.contains("Gateway sidecar cleanup"), "{cleanup}");
    assert!(cleanup.contains("node disappeared"), "{cleanup}");
}

#[test]
fn stage_failure_is_followed_by_cleanup() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let mut runner =
        FakeRunner::with_responses(vec![success(""), failure("copy failed"), success("")]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some("x86_64-unknown-linux-gnu".to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("staging failure");

    assert!(error.primary.contains("Gateway sidecar staging failed"));
    assert_eq!(runner.commands.len(), 3);
    assert_eq!(
        runner.commands[2].args[1..],
        strings(&["remove", "--target", "x86_64-unknown-linux-gnu"])
    );
}

#[test]
fn interruption_before_staging_does_not_run_removal() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let mut runner = FakeRunner::with_responses(vec![FakeResponse::InterruptedBeforeStart]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some("x86_64-pc-windows-msvc".to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("Gateway interruption");

    assert!(
        error.primary.contains("Gateway build interrupted"),
        "{error}"
    );
    assert_eq!(error.cleanup, None);
    assert_eq!(runner.commands.len(), 1);
}

#[test]
fn interruption_after_gateway_completion_does_not_run_removal() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let triple = "x86_64-pc-windows-msvc";
    let mut runner = FakeRunner::with_responses(vec![FakeResponse::CompletedAndInterrupted]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some(triple.to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("pre-staging interruption");

    assert!(
        error
            .primary
            .contains("Gateway sidecar staging interrupted"),
        "{error}"
    );
    assert!(
        runner
            .commands
            .iter()
            .all(|command| { command.args[1..] != strings(&["remove", "--target", triple]) })
    );
}

#[test]
fn interruption_during_staging_runs_target_cleanup_once() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let triple = "x86_64-pc-windows-msvc";
    let mut runner = FakeRunner::with_responses(vec![
        success(""),
        FakeResponse::InterruptedAfterStart,
        success(""),
    ]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some(triple.to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("staging interruption");

    assert!(
        error
            .primary
            .contains("Gateway sidecar staging interrupted"),
        "{error}"
    );
    let removals = runner
        .commands
        .iter()
        .filter(|command| command.args[1..] == strings(&["remove", "--target", triple]))
        .count();
    assert_eq!(removals, 1);
}

#[test]
fn interruption_raced_with_completion_runs_cleanup_once() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let triple = "x86_64-pc-windows-msvc";
    let mut runner = FakeRunner::with_responses(vec![
        success(""),
        success(""),
        FakeResponse::CompletedAndInterrupted,
        success(""),
    ]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some(triple.to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("Workshop completion race");

    assert!(
        error.primary.contains("Workshop build interrupted"),
        "{error}"
    );
    let removals = runner
        .commands
        .iter()
        .filter(|command| command.args[1..] == strings(&["remove", "--target", triple]))
        .count();
    assert_eq!(removals, 1);
}

#[test]
fn interruption_preserves_cleanup_failure_diagnostics() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let mut runner = FakeRunner::with_responses(vec![
        success(""),
        success(""),
        FakeResponse::InterruptedAfterStart,
        failure("remove broke"),
    ]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some("x86_64-unknown-linux-gnu".to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("Workshop interruption and cleanup failure");

    assert!(
        error.primary.contains("Workshop build interrupted"),
        "{error}"
    );
    let cleanup = error.cleanup.expect("cleanup diagnostic");
    assert!(
        cleanup.contains("Gateway sidecar cleanup failed"),
        "{cleanup}"
    );
    assert!(cleanup.contains("remove broke"), "{cleanup}");
}

#[test]
fn cleanup_failure_after_success_fails_the_command_clearly() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let mut runner = FakeRunner::with_responses(vec![
        success(""),
        success(""),
        success(""),
        failure("remove failed"),
    ]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: Some("x86_64-unknown-linux-gnu".to_owned()),
        },
        environment,
        &mut runner,
    )
    .expect_err("cleanup failure");

    assert!(
        error.primary.contains("Gateway sidecar cleanup failed"),
        "{error}"
    );
    assert!(error.primary.contains("remove failed"), "{error}");
    assert_eq!(error.cleanup, None);
}

#[test]
fn malformed_host_output_fails_before_building() {
    let test_environment = environment();
    let environment = &test_environment.environment;
    let mut runner = FakeRunner::with_responses(vec![success("cargo 1.89.0\n")]);

    let error = build_workshop(
        &BuildRequest {
            profile: Profile::Debug,
            target: None,
        },
        environment,
        &mut runner,
    )
    .expect_err("missing host");

    assert!(error.primary.contains("Cargo host triple"), "{error}");
    assert_eq!(runner.commands.len(), 1);
}
