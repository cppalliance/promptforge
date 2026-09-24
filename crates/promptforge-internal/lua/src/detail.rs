//! Operations on the Lua boundary's types that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host.

use crate::SharedSource;

/// Wraps a concrete error as a shareable cause.
#[must_use]
pub fn shared_source_new(source: impl std::error::Error + Send + Sync + 'static) -> SharedSource {
    SharedSource(std::sync::Arc::new(source))
}
