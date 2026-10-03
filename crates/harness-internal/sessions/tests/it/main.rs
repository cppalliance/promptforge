//! Integration tests for `harness-sessions`: the session runtime end to
//! end on an in-process Harness over a temporary agents directory, and a
//! prepared run driven through the effect loop, each on a scripted or
//! offline inference broker.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod end_to_end;
mod session;
mod support;
