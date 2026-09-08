//! Workshop server integration tests that pin wire behavior end to end,
//! one module per socket concern.

#[path = "../common/mod.rs"]
mod common;

mod agents;
mod chat_gate;
mod heartbeat;
mod observer;
mod realtime_relay;
mod session;
