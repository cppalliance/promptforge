//! Integration tests for `harness-models`: a prepared run driven end to
//! end through the effect loop, with this crate's chat performer against
//! an axum mock gateway and an in-memory run log.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod end_to_end;
