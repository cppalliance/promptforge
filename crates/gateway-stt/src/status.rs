//! Point-in-time speech service status.

/// Generic facts about the active speech generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeechStatus {
    configured: bool,
    ready: bool,
    gpu: bool,
    generation: Option<u64>,
}

impl SpeechStatus {
    pub(crate) const fn inactive() -> Self {
        Self {
            configured: false,
            ready: false,
            gpu: false,
            generation: None,
        }
    }

    pub(crate) const fn active(gpu: bool, generation: u64) -> Self {
        Self {
            configured: true,
            ready: true,
            gpu,
            generation: Some(generation),
        }
    }

    /// Returns whether the active profile configures speech.
    #[must_use]
    pub const fn configured(self) -> bool {
        self.configured
    }

    /// Returns whether one complete generation accepts requests.
    #[must_use]
    pub const fn ready(self) -> bool {
        self.ready
    }

    /// Returns whether the active backend reports GPU acceleration.
    #[must_use]
    pub const fn gpu(self) -> bool {
        self.gpu
    }

    /// Returns the active generation identifier.
    #[must_use]
    pub const fn generation(self) -> Option<u64> {
        self.generation
    }
}
