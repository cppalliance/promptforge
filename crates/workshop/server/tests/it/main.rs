//! Workshop server integration tests, one module per behavior area: the
//! `/ws` socket and its heartbeat-driven frames, the `/agents/ws` socket
//! and the built-in chat agent's parity gate, the realtime relay, the
//! heartbeat loop, boot composition, the save timeout, the user-state
//! bucket, and the workspace file across a graceful shutdown.

#[path = "../common/mod.rs"]
mod common;

mod agents;
mod boot;
mod chat_gate;
mod heartbeat;
mod heartbeat_loop;
mod realtime_relay;
mod save_timeout;
mod session;
mod user_state;
mod workspace_shutdown;
