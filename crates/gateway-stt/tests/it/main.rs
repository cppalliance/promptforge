//! STT HTTP and WebSocket integration tests.

#[cfg(not(miri))]
#[path = "../common/mod.rs"]
mod common;

#[cfg(not(miri))]
mod architecture;
#[cfg(not(miri))]
mod batch;
#[cfg(not(miri))]
mod generation;
#[cfg(not(miri))]
mod realtime_fixtures;
#[cfg(not(miri))]
mod realtime_session;
#[cfg(not(miri))]
mod service;
