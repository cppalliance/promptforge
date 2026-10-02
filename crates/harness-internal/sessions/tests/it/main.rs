//! Integration tests for `harness-sessions`: the session runtime end to
//! end on an in-process Harness over a temporary agents directory, and a
//! prepared run driven through the effect loop with the chat performer
//! against an axum mock gateway.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod end_to_end;
mod session;
