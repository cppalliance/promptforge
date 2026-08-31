//! Shared slot for the active STT engine.

use std::sync::{Arc, PoisonError, RwLock};

use crate::engine::SttEngine;

/// Shared holder for the active STT engine.
///
/// Reads happen per request and writes happen on profile switches, so a
/// standard [`RwLock`] suffices. No guard crosses an `.await`, and lock
/// poisoning recovers the value so a panicking peer cannot wedge STT for
/// the process lifetime.
#[derive(Debug, Clone, Default)]
pub struct SttSlot {
    engine: Arc<RwLock<Option<Arc<SttEngine>>>>,
}

impl SttSlot {
    /// The engine, when it has loaded.
    #[must_use]
    pub fn engine(&self) -> Option<Arc<SttEngine>> {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the engine has loaded.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Installs a loaded engine.
    pub fn activate(&self, engine: SttEngine) {
        *self.engine.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(engine));
    }

    /// Removes and drops the active engine.
    ///
    /// Returns whether an engine was active.
    #[must_use]
    pub fn deactivate(&self) -> bool {
        self.take().is_some()
    }

    /// Removes and returns the active engine.
    ///
    /// The runtime uses the returned strong handle to wait until route
    /// borrowers release the engine before loading replacement model memory.
    #[must_use]
    pub fn take(&self) -> Option<Arc<SttEngine>> {
        self.engine
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_slot_deactivates_without_work() {
        let slot = SttSlot::default();
        assert!(!slot.is_active());
        assert!(!slot.deactivate());
        assert!(!slot.is_active());
    }
}
