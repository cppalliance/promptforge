//! harness-api - the public door into the PromptForge harness family: the
//! harness configuration, the gateway binding a client pushes at startup
//! and on every gateway replacement, and the session, event, and delta
//! types a client renders.
//!
//! ## Invariants
//!
//! - Family: harness door; may depend on: `promptforge-api-runtime`,
//!   `promptforge-api-types`, `gateway-api`, `gateway-api-discovery`,
//!   `shared-*`, and the crates under `crates/harness/`. Never on a
//!   `workshop-*` crate or a private `gateway-*` crate. Read `AGENTS.md`
//!   before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - A gateway bearer key is never written to logs or `Debug` output.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

pub mod bridge;
mod harness;
mod session;

pub use harness::{GatewayBinding, Harness, HarnessConfig};
pub use session::{Delta, DeltaKind, LaunchRequest, Session, SessionEvent, SessionId};
