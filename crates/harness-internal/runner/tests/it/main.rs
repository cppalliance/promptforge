//! Integration tests for `harness-runner`: the per-run Harness against a
//! scripted Host, the effect loop against fake performers and an
//! in-memory recorder, the runner's own performers under the loop, and
//! run preparation.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod effect_loop;
mod harness;
mod performers;
mod prepare;
mod recorder;
mod scripted;
mod support;
mod tool_context;
