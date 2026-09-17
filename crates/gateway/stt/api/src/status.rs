//! Point-in-time speech service status.

/// Generic facts about the published speech runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeechStatus {
    configured: bool,
    ready: bool,
    gpu: bool,
}

impl SpeechStatus {
    pub(crate) const fn unready(configured: bool) -> Self {
        Self {
            configured,
            ready: false,
            gpu: false,
        }
    }

    pub(crate) const fn active(gpu: bool) -> Self {
        Self {
            configured: true,
            ready: true,
            gpu,
        }
    }

    /// Returns whether the active profile configures speech.
    #[must_use]
    pub const fn configured(self) -> bool {
        self.configured
    }

    /// Returns whether the published runtime accepts requests.
    #[must_use]
    pub const fn ready(self) -> bool {
        self.ready
    }

    /// Returns whether the active backend reports GPU acceleration.
    #[must_use]
    pub const fn gpu(self) -> bool {
        self.gpu
    }
}
