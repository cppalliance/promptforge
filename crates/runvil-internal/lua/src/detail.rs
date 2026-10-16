//! The constructor the compiled-program statics wrap their failure with.

use crate::SharedSource;

/// Wraps a concrete error as a shareable cause.
#[must_use]
pub(crate) fn shared_source_new(
    source: impl std::error::Error + Send + Sync + 'static,
) -> SharedSource {
    SharedSource(std::sync::Arc::new(source))
}
