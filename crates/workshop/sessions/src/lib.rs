//! workshop-sessions - the sessions subsystem: the `/ws` workbench
//! socket (status, catalog, and workbench snapshots downstream,
//! Model-menu events inbound), the `/agents/ws` agent-session socket
//! with its run supervision and operator input waits, and the
//! `/v1/models` buffered catalog relay.
//!
//! ## Invariants
//!
//! - Tier: feature; may depend on: `workshop-protocol`,
//!   `workshop-registry`, `workshop-support`, the service crates
//!   (`workshop-gateway`, `workshop-menu`, `workshop-status`), the
//!   promptforge door (with its `test-support` feature: the tokio test
//!   driver is the sessions' interim run host until the harness lands),
//!   and the harness door `harness-api` (the engine's model client and
//!   capability registry are named through its `bridge`). Read
//!   `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - One task owns each socket: a single `select!` loop reads inbound
//!   frames and writes every outbound frame itself - no outbox channel,
//!   no writer task. The agent-session registry is the documented
//!   carve-out, because sessions outlive sockets on purpose.
//! - The workspace's granted roots are read through the registry's
//!   `WorkspaceRoots` slot, never by naming the workspace crate: feature
//!   crates in the same tier meet through the registry.
//! - The shell's WebSocket origin policy is injected into
//!   [`SessionsState`] as a plain function and applied to every upgrade;
//!   the cross-site guard stays the shell's security boundary.
//! - A dying input wait is an outcome, never silence: every path out of
//!   an unresolved wait removes the entry and pushes a durable
//!   `input_cancelled` frame.

pub mod agents;
pub mod input;
mod relay;
mod session;
pub mod state;

pub use agents::{AgentSessions, SessionHost, session_registry};
pub use input::{SessionInputBroker, WaitError, WaitRegistry};
pub use state::{SessionsState, register, routes};
