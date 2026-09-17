//! Workshop server integration tests that pin wire behavior end to end,
//! one module per socket concern.

#[path = "../common/mod.rs"]
mod common;

mod agents;
mod boot;
mod chat_gate;
mod heartbeat;
mod heartbeat_loop;
mod observer;
mod realtime_relay;
mod session;
mod user_state;
mod workspace_shutdown;
