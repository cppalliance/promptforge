//! Bounded, prioritized file logging for the PromptForge gateway.
//!
//! [`LogRuntime`] owns one worker thread that drains a fixed-capacity
//! priority queue into a rotated `gateway.log`; [`LogWriter`] adapts the
//! queue to `tracing-subscriber`'s `MakeWriter` so the binary's fmt layer
//! enqueues formatted events instead of blocking producer threads on disk.
//!
//! The crate never installs the global subscriber, never reads the
//! environment or the home directory, and never sees Gateway configuration:
//! the caller passes the state directory in through [`LogConfig`] and
//! composes the subscriber itself.

mod config;
mod error;
mod queue;
mod runtime;
mod worker;
mod writer;

pub use crate::config::LogConfig;
pub use crate::error::LogError;
pub use crate::runtime::LogRuntime;
pub use crate::writer::LogWriter;
// Forced onto the public surface by E0446: `MakeWriter::Writer` cannot
// name a private type. Hidden and not part of the API contract.
#[doc(hidden)]
pub use crate::writer::LogEventWriter;
