//! The facade's integration suite, written against `promptforge` paths
//! only: the auto traits the API promises, pinned at their facade paths,
//! and the offline prompt-fixture cases that need only the public API -
//! parse-error contracts, shipped-prompt policy, and the prepare pass -
//! over shared test support in [`support`]. The fixture cases that reach
//! items outside the facade run in `promptforge-engine`'s own suite.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod auto_traits;
mod cancel;
mod capabilities;
mod effect;
mod event;
mod greeter;
mod ids;
mod metrics;
mod model;
mod parsing;
mod prepare;
mod prompt;
mod replay;
mod shipped;
mod support;
mod timestamp;
mod tools;
mod vfs;
