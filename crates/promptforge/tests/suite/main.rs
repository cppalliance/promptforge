//! The facade's integration suite, written against `promptforge` paths
//! only: the auto traits the API promises, pinned at their facade paths,
//! and the offline prompt-fixture cases that need nothing outside the host
//! surface - parse-error contracts, shipped-prompt policy, and the prepare
//! pass - over shared harness code in [`support`]. The fixture cases that
//! reach engine-only items run in the engine's own suite.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod auto_traits;
mod parsing;
mod prepare;
mod shipped;
mod support;
