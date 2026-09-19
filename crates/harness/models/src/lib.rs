//! harness-models - the harness's model client: the HTTP transport that
//! performs the engine's `Chat` effects against the bound gateway and
//! streams deltas back to the session.
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
//! - A gateway bearer key is never written to logs or `Debug` output.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).
