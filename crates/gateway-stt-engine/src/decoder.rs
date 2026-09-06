//! Backend-neutral model construction and stateless decoding contracts.

use std::fmt::Debug;

use crate::TranscribeError;

/// One backend decoder confined to a transcription worker thread.
///
/// Every call receives all guidance and finalized history needed for that
/// decode. Implementations must not retain request state between calls.
pub trait Decoder {
    /// Decodes one owned worker job.
    ///
    /// # Errors
    /// Returns a backend-translated transcription failure.
    fn transcribe(
        &mut self,
        samples: &[f32],
        guidance: &[String],
        finalized: &str,
    ) -> Result<String, TranscribeError>;
}

/// Constructs backend decoders on the worker threads that own them.
pub trait ModelFactory: Debug + Send + Sync + 'static {
    /// Constructs the required interim decoder.
    ///
    /// # Errors
    /// Returns a backend-translated model construction failure.
    fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError>;

    /// Constructs the optional final decoder.
    ///
    /// `None` means final-pass transcription is not configured.
    ///
    /// # Errors
    /// Returns a backend-translated model construction failure.
    fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError>;

    /// Whether the loaded backend reports hardware acceleration.
    fn gpu_available(&self) -> bool;
}
