//! Backend-neutral model construction and stateless decoding contracts.

use std::fmt::{self, Debug};
use std::sync::Arc;

use crate::TranscribeError;

/// Selects the physical worker and backend decode policy for one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeMode {
    /// Responsive provisional transcription.
    Interim,
    /// Accurate authoritative transcription.
    Final,
}

/// One complete stateless decode job.
#[derive(Clone, Debug)]
pub struct DecodeRequest {
    mode: DecodeMode,
    samples: Vec<f32>,
    guidance: Vec<String>,
    finalized: String,
    lifetime_guard: Option<RequestLifetime>,
}

#[derive(Clone)]
struct RequestLifetime(Arc<dyn Send + Sync>);

impl fmt::Debug for RequestLifetime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestLifetime")
    }
}

impl DecodeRequest {
    /// Creates one owned decode request.
    #[must_use]
    pub fn new(
        mode: DecodeMode,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Self {
        Self {
            mode,
            samples,
            guidance,
            finalized,
            lifetime_guard: None,
        }
    }

    /// Keeps `guard` alive until the worker retires this request.
    #[must_use]
    pub fn with_lifetime_guard(mut self, guard: impl Send + Sync + 'static) -> Self {
        self.lifetime_guard = Some(RequestLifetime(Arc::new(guard)));
        self
    }

    /// Requested worker and decode policy.
    #[must_use]
    pub fn mode(&self) -> DecodeMode {
        self.mode
    }

    /// Owned mono 16 kHz floating-point PCM.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Immutable user guidance for this job.
    #[must_use]
    pub fn guidance(&self) -> &[String] {
        &self.guidance
    }

    /// Finalized transcript history for this job.
    #[must_use]
    pub fn finalized(&self) -> &str {
        &self.finalized
    }

    pub(crate) fn take_lifetime_guard(&mut self) -> Option<Arc<dyn Send + Sync>> {
        self.lifetime_guard.take().map(|guard| guard.0)
    }
}

/// One backend decoder confined to a transcription worker thread.
///
/// Implementations must not retain request state between calls.
pub trait Decoder {
    /// Decodes one owned worker job.
    ///
    /// # Errors
    /// Returns a backend-translated transcription failure.
    fn decode(&mut self, request: DecodeRequest) -> Result<String, TranscribeError>;
}

/// Constructs backend decoders on the worker threads that own them.
pub trait ModelFactory: Debug + Send + Sync + 'static {
    /// Constructs the decoder for `mode`.
    ///
    /// `None` is valid only for an unconfigured final worker.
    ///
    /// # Errors
    /// Returns a backend-translated model construction failure.
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError>;
}
