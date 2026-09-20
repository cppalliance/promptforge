//! Integration tests for `harness-runner`: the effect loop against fake
//! performers and an in-memory log, the runner's own performers under the
//! loop, and the tagged spawn wrappers.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod effect_loop;
mod performers;
mod spawn;
mod support;
