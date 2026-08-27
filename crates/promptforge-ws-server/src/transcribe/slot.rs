//! One-shot shared slot that fills with the voice engine once it loads.

use std::sync::{Arc, PoisonError, RwLock};

use crate::transcribe::engine::VoiceEngine;

/// Shared holder for the voice engine: empty until the engine loads, then
/// filled exactly once - at startup from local model files, or later by the
/// provisioning task once the gateway cache has provided them.
///
/// Reads happen per `/voice` session upgrade and writes are one-shot, so a
/// std `RwLock` suffices; no guard ever crosses an `.await`. Lock poisoning
/// recovers the value, matching the tape's posture: a panicking writer
/// cannot wedge voice for the process's life.
#[derive(Debug, Clone, Default)]
pub(crate) struct VoiceSlot {
    engine: Arc<RwLock<Option<Arc<VoiceEngine>>>>,
}

impl VoiceSlot {
    /// The engine, when it has loaded.
    pub(crate) fn engine(&self) -> Option<Arc<VoiceEngine>> {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the engine has loaded.
    pub(crate) fn is_active(&self) -> bool {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Installs a loaded engine.
    pub(crate) fn activate(&self, engine: VoiceEngine) {
        *self.engine.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(engine));
    }
}
