//! harness-runner - the harness effect loop: prepares an engine `Run`
//! from a prompt file (drawing the host inputs the engine refuses to draw
//! itself, activating capabilities, opening the run's row), steps it,
//! performs each effect on tokio through one performer per effect kind,
//! feeds the answers back, records every event, effect, and answer in the
//! run log, and owns cancellation.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge-api-runtime`, `promptforge-api-types`, `gateway-api`,
//!   `gateway-api-discovery`, `shared-*`, and its container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   `promptforge-*` crate behind the door. Read `AGENTS.md` before adding
//!   an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - [`spawn::spawn_tagged`] and [`spawn::spawn_blocking_tagged`] are the
//!   only sites in the harness that call `tokio::spawn` and
//!   `tokio::task::spawn_blocking`; every other harness crate's
//!   `clippy.toml` bans the raw calls, and `cargo test -p build-xtask`
//!   checks the bans are declared.
//! - The log is written in loop order: a step's events before the step's
//!   effects are issued, each effect before its performer starts, each
//!   answer before the run resumes with it. Every effect record has
//!   exactly one answer record; a dropped effect's answer is `Dropped`.
//! - The loop never reads an event to decide anything; control rides on
//!   the run's own word (`Step`, `Run::decided`) and the cancel flag.
//! - [`cancel::CancelHandle`] is the awaitable token a host selects over;
//!   the engine observes only the polled flag in
//!   `promptforge_api_types::cancel`, and a host bridges the one to the
//!   other when it launches a run. `harness-api` re-exports the module.

pub mod cancel;
pub mod effect_loop;
pub mod performers;
pub mod prepare;
pub mod spawn;
#[cfg(feature = "test-support")]
pub mod test_support;
