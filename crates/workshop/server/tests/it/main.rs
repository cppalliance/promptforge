//! Workshop server integration tests, one module per behavior area: the
//! `/ws` socket and its heartbeat-driven frames, the `/agents/ws` socket
//! and the built-in chat agent's parity gate, the realtime relay, the
//! heartbeat loop, boot composition, the user-state bucket, the
//! workspace file across a graceful shutdown, and the workspace save
//! deadline through the full router.

#[path = "../common/mod.rs"]
mod common;

mod agents;
mod boot;
mod chat_gate;
mod heartbeat;
mod heartbeat_loop;
mod realtime_relay;
mod user_state;
mod workshop_socket;
mod workspace_shutdown;
mod workspace_timeout;
