//! The tagged spawn wrappers run their work to completion.

use harness_runner::spawn::{spawn_blocking_tagged, spawn_tagged};

#[tokio::test]
async fn spawn_tagged_runs_a_future_to_completion() {
    let handle = spawn_tagged("effect-7", async { 6 * 7 });
    let value = handle.await.expect("the spawned future completes");
    assert_eq!(value, 42);
}

#[tokio::test]
async fn spawn_blocking_tagged_runs_a_closure_to_completion() {
    let handle = spawn_blocking_tagged("store-3", || "done".repeat(2));
    let value = handle.await.expect("the blocking closure completes");
    assert_eq!(value, "donedone");
}

#[tokio::test]
async fn spawn_tagged_accepts_any_display_tag() {
    let tag = format!("task-{}/{}", 0, 12);
    let handle = spawn_tagged(tag, async { true });
    assert!(handle.await.expect("the spawned future completes"));
}
