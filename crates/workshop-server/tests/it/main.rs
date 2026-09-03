//! The workshop server's integration-test binary: characterization tests
//! that pin the workshop wire behavior end to end, one module per
//! socket concern, plus the module size ratchet guarding src/ structure
//! and the persisted event-log schema canary.

#[path = "../common/mod.rs"]
mod common;

mod agents;
mod chat_gate;
mod heartbeat;
mod observer;
mod ratchet;
mod session;
