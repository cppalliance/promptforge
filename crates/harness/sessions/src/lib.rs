//! harness-sessions - the harness session layer: agent discovery, session
//! state, the input wait registry, and the supervisor state machine that
//! takes a run from alive through closing to closed.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge-api-runtime`, `promptforge-api-types`, `gateway-api`,
//!   `gateway-api-discovery`, `shared-*`, and its container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   `promptforge-*` crate behind the door. Read `AGENTS.md` before adding
//!   an import.
//! - The supervisor's state transitions are a pure reducer whose matches
//!   stay wildcard-free, so a new variant is a compile error.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).
