//! Registry tests.
//!
//! Every one of them runs on Tokio's paused clock, so a thirty-second admission
//! wait and a one-hour retention window cost the suite nothing and neither
//! depends on how busy the machine is.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;

use super::RunRegistry;
use crate::config::Config;
use crate::levels::Levels;
use crate::result::{RunResult, RunStatus};

/// A registry over a `[server]` table carrying `lines` on top of the defaults.
fn registry(lines: &str) -> Arc<RunRegistry> {
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n{lines}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n"
    ))
    .expect("the fixture configuration parses");
    Arc::new(RunRegistry::new(&config.server))
}

/// A level recorder installed for the rest of the calling thread, which is also
/// where a current-thread runtime polls every task the registry spawns.
///
/// The guard the caller holds is what uninstalls it.
fn recording() -> (Levels, tracing::subscriber::DefaultGuard) {
    let levels = Levels::default();
    let recorder = tracing_subscriber::registry().with(levels.clone());
    (levels, tracing::subscriber::set_default(recorder))
}

/// The result a finished run of `prompt` leaves behind.
fn completed(run_id: &str, prompt: &str) -> RunResult {
    RunResult::completed(run_id.to_owned(), prompt, 1, "done".to_owned(), 2, 40)
}

#[tokio::test(start_paused = true)]
async fn a_run_that_outlives_the_deadline_keeps_going_and_is_collected_later() {
    let registry = registry("reply_deadline = \"50ms\"");
    registry.started("r1", "slow", 1);
    let task = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let result = completed("r1", "slow");
            registry.finished("r1", result.clone());
            result
        }
    });

    let started = Instant::now();
    let reply = registry.settle("r1", "slow", 1, task).await;
    assert_eq!(started.elapsed(), Duration::from_millis(50));
    assert_eq!(reply.status, RunStatus::Running);
    assert_eq!(reply.run_id, "r1");
    assert!(reply.value.is_none());

    // The deadline dropped the handle rather than aborting the task, so the run
    // is still going and its value arrives at the registry on its own time.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let collected = registry
        .check("r1")
        .expect("the finished run is collectable");
    assert_eq!(collected.status, RunStatus::Completed);
    assert_eq!(collected.value.as_deref(), Some("done"));
}

#[tokio::test(start_paused = true)]
async fn a_run_inside_the_deadline_answers_with_its_own_result() {
    let registry = registry("reply_deadline = \"5s\"");
    registry.started("r1", "quick", 1);
    let task = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            let result = completed("r1", "quick");
            registry.finished("r1", result.clone());
            result
        }
    });

    let reply = registry.settle("r1", "quick", 1, task).await;
    assert_eq!(reply.status, RunStatus::Completed);
    assert_eq!(reply.value.as_deref(), Some("done"));
}

#[tokio::test(start_paused = true)]
async fn a_run_that_panicked_is_reported_as_a_failure_and_recorded_as_one() {
    let registry = registry("");
    registry.started("r1", "doomed", 1);
    let task = tokio::spawn(async { panic!("the run's own bug") });

    let reply = registry.settle("r1", "doomed", 1, task).await;
    assert_eq!(reply.status, RunStatus::Failed);
    assert_eq!(
        registry.check("r1").map(|run| run.status),
        Some(RunStatus::Failed),
        "a caller that polls instead of waiting learns the same thing"
    );
}

#[tokio::test(start_paused = true)]
async fn a_backgrounded_run_that_panics_becomes_a_collectable_failure() {
    let registry = registry("reply_deadline = \"50ms\"\nretain_completed = \"1h\"");
    registry.started("r1", "doomed", 1);
    let task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        panic!("the run's own bug")
    });

    let reply = registry.settle("r1", "doomed", 1, task).await;
    assert_eq!(
        reply.status,
        RunStatus::Running,
        "the call gave up on it before it died"
    );

    // The supervisor owns the handle the deadline let go of, so the panic still
    // reaches the record instead of leaving it running for ever.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let collected = registry
        .check("r1")
        .expect("a run that died is still collectable");
    assert_eq!(collected.status, RunStatus::Failed);
    let error = collected.error.expect("a failure says why");
    assert!(error.contains("did not finish"), "{error}");

    // And a terminal record is an evictable one, which a `running` record is not.
    tokio::time::sleep(Duration::from_secs(3601)).await;
    assert!(
        registry.check("r1").is_none(),
        "the record ages out like any other finished run"
    );
}

#[tokio::test(start_paused = true)]
async fn a_backgrounded_run_logs_the_id_it_handed_back_and_the_outcome_that_followed() {
    let (levels, _recording) = recording();
    let registry = registry("reply_deadline = \"50ms\"");
    registry.started("r1", "slow", 1);
    let task = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let result = completed("r1", "slow");
            registry.finished("r1", result.clone());
            result
        }
    });

    let reply = registry.settle("r1", "slow", 1, task).await;
    assert_eq!(reply.status, RunStatus::Running);
    tokio::time::sleep(Duration::from_secs(6)).await;

    assert_eq!(
        levels.operator_visible(),
        vec![Level::INFO, Level::INFO],
        "the run leaving its call and the run ending, and nothing else: {levels:?}"
    );
    assert!(
        levels.said(Level::INFO, "outlived its call"),
        "the id the caller was handed: {levels:?}"
    );
    assert!(
        levels.said(Level::INFO, "terminal state"),
        "the outcome nobody else observed: {levels:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_call_is_refused_once_every_slot_is_taken_and_admitted_when_one_returns() {
    let (levels, _recording) = recording();
    let registry = registry("max_concurrent_runs = 1\nadmission_timeout = \"30s\"");
    let slot = registry.admit().await.expect("the only slot is free");

    let started = Instant::now();
    assert!(registry.admit().await.is_none(), "the slot is taken");
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(30),
        "a refusal costs exactly the configured wait"
    );

    assert_eq!(
        levels.operator_visible(),
        vec![Level::WARN],
        "a refusal is what the concurrency limit looks like from outside: {levels:?}"
    );
    assert!(
        levels.said(Level::WARN, "no run slot came free"),
        "{levels:?}"
    );

    drop(slot);
    assert!(
        registry.admit().await.is_some(),
        "a slot returns when the run holding it ends"
    );
}

#[tokio::test(start_paused = true)]
async fn a_finished_record_is_evicted_once_its_window_passes() {
    let registry = registry("retain_completed = \"1h\"");
    registry.started("r1", "echo", 1);
    registry.finished("r1", completed("r1", "echo"));
    assert!(registry.check("r1").is_some(), "inside the window");

    tokio::time::sleep(Duration::from_secs(3601)).await;
    assert!(
        registry.check("r1").is_none(),
        "past the window the record is gone, and an id nobody kept is unknown"
    );
}

#[tokio::test(start_paused = true)]
async fn a_running_record_outlives_the_retention_window() {
    let registry = registry("retain_completed = \"1h\"");
    registry.started("r1", "long", 1);

    tokio::time::sleep(Duration::from_secs(7200)).await;
    let running = registry
        .check("r1")
        .expect("a run still going is never evicted");
    assert_eq!(running.status, RunStatus::Running);
    assert_eq!(
        running.elapsed_ms, 7_200_000,
        "a running run reports how long it has been going"
    );
}

#[tokio::test(start_paused = true)]
async fn an_id_no_run_ever_had_is_unknown() {
    let registry = registry("");
    assert!(registry.check("nonesuch").is_none());
}
