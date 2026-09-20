//! Integration tests for `harness-sessions`: the session runtime end to
//! end on an in-process harness over a temporary state directory.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod session;
