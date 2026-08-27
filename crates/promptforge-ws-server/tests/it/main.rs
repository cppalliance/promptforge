//! The workshop server's integration-test binary: characterization tests
//! that pin the chat and voice wire behavior end to end, as it stands
//! before the session rewrite, one module per socket concern.

#[path = "../common/mod.rs"]
mod common;

mod chat;
mod heartbeat;
mod voice;
