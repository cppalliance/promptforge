//! Registry tests.
//!
//! Every one of them runs on Tokio's paused clock, so a thirty-second admission
//! wait and a one-hour retention window cost the suite nothing and neither
//! depends on how busy the machine is.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use promptforge_core::CancelHandle;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tracing::Level;

use super::RunRegistry;
use crate::config::Config;
use crate::levels::recording;
use crate::result::{NO_TURNS, RunResult, RunStatus};

/// A registry over a `[server]` table carrying `lines` on top of the defaults.
fn registry(lines: &str) -> Arc<RunRegistry> {
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n{lines}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n"
    ))
    .expect("the fixture configuration parses");
    Arc::new(RunRegistry::new(&config.server))
}

/// The result a finished run of `prompt` leaves behind.
fn completed(run_id: &str, prompt: &str) -> RunResult {
    RunResult::completed(run_id.to_owned(), prompt, "done".to_owned(), 2, 40)
}

/// Registers `run_id` running `body`, returning the result receiver a caller
/// settles the run with. The registry holds the run's cancellation handle.
fn launch(
    registry: &Arc<RunRegistry>,
    run_id: &str,
    prompt: &str,
    body: impl Future<Output = RunResult> + Send + 'static,
) -> oneshot::Receiver<RunResult> {
    let (tx, rx) = oneshot::channel();
    registry
        .launch(
            run_id.to_owned(),
            prompt.to_owned(),
            CancelHandle::new(),
            tx,
            move || tokio::spawn(body),
        )
        .expect("a fresh run id registers");
    rx
}

#[tokio::test(start_paused = true)]
async fn a_run_that_outlives_the_deadline_keeps_going_and_is_collected_later() {
    let registry = registry("reply_deadline = \"50ms\"");
    let rx = launch(&registry, "r1", "slow", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        completed("r1", "slow")
    });

    let started = Instant::now();
    let reply = registry.settle("r1", "slow", rx).await;
    assert_eq!(started.elapsed(), Duration::from_millis(50));
    assert_eq!(reply.status(), RunStatus::Running);
    assert_eq!(reply.run_id(), "r1");
    assert!(reply.value().is_none());

    // The deadline disarmed the cancellation guard rather than stopping the run,
    // so the run is still going and its value arrives on its own time.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let collected = registry
        .check("r1")
        .expect("the finished run is collectable");
    assert_eq!(collected.status(), RunStatus::Completed);
    assert_eq!(collected.value(), Some("done"));
}

#[tokio::test(start_paused = true)]
async fn a_run_inside_the_deadline_answers_with_its_own_result() {
    let registry = registry("reply_deadline = \"5s\"\nretain_completed = \"1h\"");
    let rx = launch(&registry, "r1", "quick", async { completed("r1", "quick") });

    let reply = registry.settle("r1", "quick", rx).await;
    assert_eq!(reply.run_id(), "r1");
    assert_eq!(reply.prompt(), "quick");
    assert_eq!(reply.status(), RunStatus::Completed);
    assert_eq!(reply.value(), Some("done"));
    assert_eq!(reply.turns(), 2);
    assert_eq!(reply.elapsed_ms(), 40);
    assert!(reply.error().is_none());

    // A caller that polls the record instead reads the same contract.
    let stored = registry
        .check("r1")
        .expect("the finished run is collectable");
    assert_eq!(stored.run_id(), "r1");
    assert_eq!(stored.prompt(), "quick");
    assert_eq!(stored.status(), RunStatus::Completed);
    assert_eq!(stored.value(), Some("done"));
    assert_eq!(stored.turns(), 2);
    assert_eq!(stored.elapsed_ms(), 40);
    assert!(stored.error().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_run_that_panicked_is_reported_as_a_failure_and_recorded_as_one() {
    let registry = registry("reply_deadline = \"5s\"");
    let rx = launch(&registry, "r1", "doomed", async {
        panic!("the run's own bug")
    });

    let reply = registry.settle("r1", "doomed", rx).await;
    assert_eq!(reply.run_id(), "r1");
    assert_eq!(reply.prompt(), "doomed");
    assert_eq!(reply.status(), RunStatus::Failed);
    assert!(reply.value().is_none());
    assert_eq!(reply.turns(), NO_TURNS);
    let error = reply.error().expect("a failure says why");
    assert!(error.contains("did not finish"), "{error}");

    let stored = registry
        .check("r1")
        .expect("a caller that polls instead of waiting learns the same thing");
    assert_eq!(stored.run_id(), "r1", "the stored record keeps its id");
    assert_eq!(stored.prompt(), "doomed", "and the prompt it was about");
    assert_eq!(stored.status(), RunStatus::Failed);
    assert!(stored.value().is_none());
    assert_eq!(stored.turns(), NO_TURNS);
    assert_eq!(
        stored.elapsed_ms(),
        0,
        "a run that reached no turn is timed from its own start, which the paused clock never advanced"
    );
    assert!(
        stored.error().is_some_and(|e| e.contains("did not finish")),
        "{stored:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_backgrounded_run_that_panics_becomes_a_collectable_failure() {
    let registry = registry("reply_deadline = \"50ms\"\nretain_completed = \"1h\"");
    let rx = launch(&registry, "r1", "doomed", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        panic!("the run's own bug")
    });

    let reply = registry.settle("r1", "doomed", rx).await;
    assert_eq!(
        reply.status(),
        RunStatus::Running,
        "the call gave up on it before it died"
    );

    // The supervisor has owned the handle since the run started, so the panic
    // still reaches the record instead of leaving it running for ever.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let collected = registry
        .check("r1")
        .expect("a run that died is still collectable");
    assert_eq!(collected.status(), RunStatus::Failed);
    let error = collected.error().expect("a failure says why");
    assert!(error.contains("did not finish"), "{error}");

    // And a terminal record is an evictable one, which a `running` record is not.
    tokio::time::sleep(Duration::from_secs(3601)).await;
    assert!(
        registry.check("r1").is_none(),
        "the record ages out like any other finished run"
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_the_call_stops_the_run_and_leaves_a_collectable_failure() {
    let registry = registry("reply_deadline = \"60s\"\nretain_completed = \"1h\"");
    let cancel = CancelHandle::new();
    let (tx, rx) = oneshot::channel();
    let run_cancel = cancel.clone();
    registry
        .launch(
            "r1".to_owned(),
            "slow".to_owned(),
            cancel.clone(),
            tx,
            move || {
                tokio::spawn(async move {
                    // The run observes the cancellation and stops, ending as the
                    // failure the core would report.
                    run_cancel.cancelled().await;
                    RunResult::failed(
                        "r1".to_owned(),
                        "slow",
                        "the run did not finish: cancelled".to_owned(),
                        NO_TURNS,
                        0,
                    )
                })
            },
        )
        .expect("a fresh run id registers");

    // The awaiting call is abandoned before the reply deadline, which drops the
    // settle future and its cancellation guard.
    let abandoned =
        tokio::time::timeout(Duration::from_secs(1), registry.settle("r1", "slow", rx)).await;
    assert!(abandoned.is_err(), "the call gave up before the deadline");

    // Dropping the wait signalled the run, which stopped and recorded a terminal
    // failure rather than staying `running` for the life of the process.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let collected = registry
        .check("r1")
        .expect("a cancelled run is collectable as a failure");
    assert_eq!(collected.status(), RunStatus::Failed);

    // The cancelled run's record ages out like any other terminal one.
    tokio::time::sleep(Duration::from_secs(3601)).await;
    assert!(
        registry.check("r1").is_none(),
        "the cancelled run's record is evicted once its window passes"
    );
}

#[tokio::test(start_paused = true)]
async fn a_duplicate_run_id_is_refused_and_leaves_the_first_run_intact() {
    let registry = registry("reply_deadline = \"60s\"");
    let _rx = launch(&registry, "r1", "first", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        completed("r1", "first")
    });

    // A second registration under the same id is refused rather than replacing
    // the live record, and its task is never spawned.
    let (tx2, _rx2) = oneshot::channel();
    let mut spawned = false;
    let duplicate = registry.launch(
        "r1".to_owned(),
        "second".to_owned(),
        CancelHandle::new(),
        tx2,
        || {
            spawned = true;
            tokio::spawn(async { completed("r1", "second") })
        },
    );
    assert!(duplicate.is_err(), "a duplicate id is refused");
    assert!(!spawned, "the refused run's task is never spawned");

    let running = registry.check("r1").expect("the first run is still known");
    assert_eq!(running.prompt(), "first");
    assert_eq!(running.status(), RunStatus::Running);
}

#[tokio::test(start_paused = true)]
async fn a_second_terminal_result_does_not_overwrite_the_first() {
    let registry = registry("reply_deadline = \"5s\"\nretain_completed = \"1h\"");
    let rx = launch(&registry, "r1", "echo", async { completed("r1", "echo") });
    let reply = registry.settle("r1", "echo", rx).await;
    assert_eq!(reply.status(), RunStatus::Completed);
    assert_eq!(reply.value(), Some("done"));

    // A late frame for the same id cannot rewrite an outcome already reported.
    registry.finished(
        "r1",
        RunResult::failed("r1".to_owned(), "echo", "too late".to_owned(), NO_TURNS, 0),
    );
    let still = registry.check("r1").expect("still recorded");
    assert_eq!(still.status(), RunStatus::Completed, "first-write-wins");
    assert_eq!(still.value(), Some("done"));
    assert!(still.error().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_registered_run_is_visible_and_reaches_a_terminal_state() {
    let registry = registry("retain_completed = \"1h\"");
    let _rx = launch(&registry, "r1", "echo", async { completed("r1", "echo") });

    // Visible the instant it is registered - a run id is never observable
    // without the owner that will drive it to a terminal state.
    let running = registry
        .check("r1")
        .expect("a registered run is visible at once");
    assert_eq!(running.run_id(), "r1");
    assert_eq!(running.status(), RunStatus::Running);

    // Its supervisor drives it to a terminal record.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        registry.check("r1").map(|run| run.status()),
        Some(RunStatus::Completed),
    );
}

#[tokio::test(start_paused = true)]
async fn a_backgrounded_run_logs_the_id_without_duplicating_the_runner_terminal_log() {
    let (levels, _recording) = recording();
    let registry = registry("reply_deadline = \"50ms\"");
    let rx = launch(&registry, "r1", "slow", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        completed("r1", "slow")
    });

    let reply = registry.settle("r1", "slow", rx).await;
    assert_eq!(reply.status(), RunStatus::Running);
    tokio::time::sleep(Duration::from_secs(6)).await;

    assert_eq!(
        levels.operator_visible(),
        vec![Level::INFO],
        "only the run leaving its call is logged by the registry: {levels:?}"
    );
    assert!(
        levels.said(
            Level::INFO,
            "message=the run outlived its call and is collectable by run id"
        ),
        "the id the caller was handed: {levels:?}"
    );
    assert!(
        !levels.said(
            Level::INFO,
            "message=a backgrounded run reached its terminal state"
        ),
        "{levels:?}"
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
        levels.said(
            Level::WARN,
            "message=no run slot came free: refusing the call, which can be retried"
        ),
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
    let registry = registry("reply_deadline = \"5s\"\nretain_completed = \"1h\"");
    let rx = launch(&registry, "r1", "echo", async { completed("r1", "echo") });
    let reply = registry.settle("r1", "echo", rx).await;
    assert_eq!(reply.status(), RunStatus::Completed);
    assert!(registry.check("r1").is_some(), "inside the window");

    // The retention window is half-open: `evict` keeps a record while the time
    // since it finished is strictly less than `retain_completed`, so it is still
    // collectable at one second before the hour but gone the instant the elapsed
    // time reaches it. The run finished at the paused clock's origin, so these
    // sleeps land the check exactly one second inside the boundary and then
    // exactly on it.
    tokio::time::sleep(Duration::from_secs(3599)).await;
    assert!(
        registry.check("r1").is_some(),
        "one second inside the window the record is still collectable"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        registry.check("r1").is_none(),
        "at the exact retention boundary the record is evicted, and an id nobody kept is unknown"
    );
}

#[tokio::test(start_paused = true)]
async fn a_running_record_outlives_the_retention_window() {
    let registry = registry("retain_completed = \"1h\"");
    let _rx = launch(&registry, "r1", "long", async {
        std::future::pending::<()>().await;
        unreachable!("a pending run never produces a result")
    });

    tokio::time::sleep(Duration::from_secs(7200)).await;
    let running = registry
        .check("r1")
        .expect("a run still going is never evicted");
    assert_eq!(running.status(), RunStatus::Running);
    assert_eq!(
        running.elapsed_ms(),
        7_200_000,
        "a running run reports how long it has been going"
    );
}

#[tokio::test(start_paused = true)]
async fn an_id_no_run_ever_had_is_unknown() {
    let registry = registry("");
    assert!(registry.check("nonesuch").is_none());
}
