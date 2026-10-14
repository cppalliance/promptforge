//! The facade's integration suite, written against `promptforge` paths
//! only: the offline prompt-fixture cases that need only the public API -
//! parse-error contracts, shipped-prompt policy, and the prepare pass -
//! over shared test support in [`support`]. The fixture cases that reach
//! items outside the facade run in `promptforge-engine`'s own suite.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod cancel;
mod effect;
mod event;
mod greeter;
mod ids;
mod model;
mod parsing;
mod plugins;
mod prepare;
mod prompt;
mod replay;
mod shipped;
mod support;
mod timestamp;
mod vfs;
