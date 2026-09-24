//! The tagged spawn wrappers run their work to completion under an
//! effect's tag.

use harness_runner::spawn::{spawn_blocking_launch, spawn_blocking_tagged, spawn_tagged};
use promptforge::Step;
use promptforge::effect::EffectId;
use promptforge::ids::Provenance;

use crate::support::run;

/// The id and provenance of a real issued effect: the one input wait a
/// `user_input()` section parks on.
fn tag() -> (EffectId, Provenance) {
    let mut run = run("return user_input()");
    let Step::Pending { mut effects, .. } = run.step() else {
        panic!("the input wait leaves the run pending");
    };
    let (id, provenance, _effect) = effects.remove(0);
    (id, provenance)
}

#[tokio::test]
async fn spawn_tagged_runs_a_future_to_completion() {
    let handle = spawn_tagged(tag(), async { 6 * 7 });
    let value = handle.await.expect("the spawned future completes");
    assert_eq!(value, 42);
}

#[tokio::test]
async fn spawn_blocking_tagged_runs_a_closure_to_completion() {
    let handle = spawn_blocking_tagged(tag(), || "done".repeat(2));
    let value = handle.await.expect("the blocking closure completes");
    assert_eq!(value, "donedone");
}

#[tokio::test]
async fn spawn_blocking_launch_runs_a_closure_to_completion() {
    let handle = spawn_blocking_launch("chat", || "walked".len());
    let value = handle.await.expect("the launch closure completes");
    assert_eq!(value, 6);
}
