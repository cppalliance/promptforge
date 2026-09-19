//! harness-runner - the harness effect loop: steps an engine `Run`,
//! performs each effect on tokio through one performer per effect kind,
//! feeds the answers back, and owns cancellation and supervision.
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

pub mod spawn;
