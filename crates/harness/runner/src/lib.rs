//! harness-runner - the harness effect loop: steps an engine `Run`,
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

pub mod effect_loop;
pub mod performers;
pub mod spawn;
#[cfg(feature = "test-support")]
pub mod test_support;
