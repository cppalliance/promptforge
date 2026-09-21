//! harness-sessions - the harness session layer: agent discovery, the
//! harness handle and its bindings, session state, the input wait
//! registry, and the supervisor state machine that takes a run from alive
//! through closing to closed.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge-api-runtime`, `promptforge-api-types`,
//!   `gateway-api-types`, `gateway-api-discovery`, `shared-*`, and its
//!   container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - Every binding a run reads arrives as data pushed through the public
//!   API (the gateway, the chat catalog, the host snapshot); this crate
//!   never resolves a gateway or reads a client's state itself. It is the
//!   one place a capability provider crate is named, at registration.
//! - The supervisor's state transitions are a pure reducer whose matches
//!   stay wildcard-free, so a new variant is a compile error.
//! - A session's transcript is the run log: the live broadcast and a
//!   transcript read agree index for index, and the reply-id stamp is one
//!   rule applied to both.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrappers in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).
//! - The input broker backs only the script-side `user_input()` function,
//!   performed as the engine's `UserInput` effect. No `user_input` tool
//!   is ever advertised to a model unless a prompt explicitly adds it.
//! - A dying input wait is an outcome, never silence: every path out of
//!   an unresolved wait removes the entry and pushes a durable cancelled
//!   frame.

pub mod discovery;
pub mod environment;
pub mod input;
pub mod lifecycle;
pub mod protocol;
pub mod runtime;
pub mod session;
pub mod transition;
