//! The workshop server's integration-test binary: characterization tests
//! that pin the chat and voice wire behavior end to end, one module per
//! socket concern, plus the module size ratchet guarding src/ structure.

#[path = "../common/mod.rs"]
mod common;

mod chat;
mod heartbeat;
mod ratchet;
mod voice;
