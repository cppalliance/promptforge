//! harness-sessions - the Harness session layer: agent discovery, the
//! Harness handle and its bindings, session state, the input wait
//! registry, and the supervisor state machine that takes a run from alive
//! through closing to closed. Each run of a session is a per-run
//! `harness_runner::Harness` of its own.
//!
//! ## Invariants
//!
//! - Family: Harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - This crate is the one place a capability provider crate is named, at
//!   registration.
//! - The supervisor's state transitions are a pure reducer whose matches
//!   stay wildcard-free, so a new variant is a compile error.
//! - The Harness records every run through the Host's recorder, and this
//!   crate opens no file or database for it.
//! - A session's transcript is held in memory: the session's recorder
//!   tee appends each event the Host's recorder took to it before the
//!   live broadcast, so a transcript read and the broadcast agree index
//!   for index, and the reply-id stamp is one rule applied once.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Only the wrappers in this crate's `spawn` module call `tokio::spawn`
//!   and `tokio::task::spawn_blocking`, each under an explicit
//!   `#[expect]`; this crate's `clippy.toml` bans the raw calls
//!   everywhere else.
//! - The input broker reaches a run only through the run's Host services
//!   under `INPUT_BROKER`, where the `promptforge/user-input` ask tool
//!   waits on it. The ask tool is never advertised to a model unless a
//!   prompt explicitly binds and adds it.
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
mod spawn;
mod timer;
pub mod transition;
