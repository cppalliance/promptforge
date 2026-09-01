//! The workshop server's integration-test binary: characterization tests
//! that pin the chat wire behavior end to end, one module per
//! socket concern, plus the module size ratchet guarding src/ structure
//! and the persisted event-log schema canary.

#[path = "../common/mod.rs"]
mod common;

mod chat;
mod heartbeat;
mod observer;
mod ratchet;
