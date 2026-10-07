//! Backend-neutral model construction and stateless decoding contracts.

use std::fmt::{self, Debug};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::TranscribeError;

/// Selects the physical worker and backend decode policy for one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DecodeMode {
    /// Responsive provisional transcription.
    Interim,
    /// Accurate authoritative transcription.
    Final,
}

/// One complete stateless decode job.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DecodeRequest {
    mode: DecodeMode,
    samples: RequestSamples,
    guidance: Vec<String>,
    finalized: String,
    lifetime_guard: Option<RequestLifetime>,
    cancellation: Option<Arc<AtomicBool>>,
}

#[derive(Clone)]
struct RequestLifetime(Arc<dyn Send + Sync>);

impl fmt::Debug for RequestLifetime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestLifetime")
    }
}

struct RequestSamples {
    values: Vec<f32>,
    retirement: Option<Box<dyn FnOnce(Vec<f32>) + Send + 'static>>,
}

impl Clone for RequestSamples {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
            retirement: None,
        }
    }
}

impl Debug for RequestSamples {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestSamples")
            .field("values", &self.values)
            .field(
                "retirement",
                &self.retirement.as_ref().map(|_| "SampleRetirement"),
            )
            .finish()
    }
}

impl Drop for RequestSamples {
    fn drop(&mut self) {
        if let Some(retire) = self.retirement.take() {
            retire(std::mem::take(&mut self.values));
        }
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
            samples: RequestSamples {
                values: samples,
                retirement: None,
            },
            guidance,
            finalized,
            lifetime_guard: None,
            cancellation: None,
        }
    }

    /// Keeps `guard` alive until the worker retires this request.
    #[must_use]
    pub fn with_lifetime_guard(mut self, guard: impl Send + Sync + 'static) -> Self {
        self.lifetime_guard = Some(RequestLifetime(Arc::new(guard)));
        self
    }

    /// Attaches the caller's cancellation flag.
    ///
    /// The flag reads true once the caller has abandoned the decode; a
    /// decoder may then stop early and fail. Clones of this request share
    /// the flag.
    #[must_use]
    pub fn with_cancellation(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(flag);
        self
    }

    /// Returns the owned sample buffer when this request retires.
    ///
    /// Cloned diagnostic snapshots never clone the callback.
    #[must_use]
    pub fn with_sample_retirement(
        mut self,
        retire: impl FnOnce(Vec<f32>) + Send + 'static,
    ) -> Self {
        self.samples.retirement = Some(Box::new(retire));
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
        &self.samples.values
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

    /// The caller's cancellation flag, if one is attached.
    ///
    /// It reads true once the caller has abandoned the decode; a decoder may
    /// then stop early and fail. Clones of this request share it.
    #[must_use]
    pub fn cancellation(&self) -> Option<&Arc<AtomicBool>> {
        self.cancellation.as_ref()
    }

    pub(crate) fn take_lifetime_guard(&mut self) -> Option<Arc<dyn Send + Sync>> {
        self.lifetime_guard.take().map(|guard| guard.0)
    }
}

/// One decode's transcript and, when its role requested them, where each of
/// its words ends.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DecodeOutput {
    text: String,
    word_ends: Vec<u64>,
}

impl DecodeOutput {
    /// A transcript without word end times.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            word_ends: Vec::new(),
        }
    }

    /// `text` with `word_ends`, which are kept only when they hold exactly
    /// one end per whitespace-delimited word of `text`.
    #[must_use]
    pub fn with_word_ends(text: impl Into<String>, mut word_ends: Vec<u64>) -> Self {
        let text = text.into();
        if word_ends.len() != text.split_whitespace().count() {
            word_ends.clear();
        }
        Self { text, word_ends }
    }

    /// The decoded transcript.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The end of each whitespace-delimited word of [`Self::text`], in
    /// samples from the request's first sample; empty when the decode
    /// produced no word timing.
    #[must_use]
    pub fn word_ends(&self) -> &[u64] {
        &self.word_ends
    }

    /// The decoded transcript, dropping any word end times.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }
}

/// One backend decoder confined to a transcription worker thread.
///
/// Implementations must not retain request state between calls.
pub trait Decoder {
    /// Decodes one owned worker job into its transcript and any word end
    /// times its role requested.
    ///
    /// # Errors
    /// Returns a backend-translated transcription failure.
    fn decode(&mut self, request: DecodeRequest) -> Result<DecodeOutput, TranscribeError>;
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::{DecodeMode, DecodeOutput, DecodeRequest};

    fn final_request() -> DecodeRequest {
        DecodeRequest::new(DecodeMode::Final, vec![1.0], Vec::new(), String::new())
    }

    #[test]
    fn miri_word_ends_are_kept_only_as_one_end_per_whitespace_delimited_word() {
        let timed = DecodeOutput::with_word_ends(" ask  not, ", vec![4_000, 9_600]);
        assert_eq!(timed.text(), " ask  not, ");
        assert_eq!(timed.word_ends(), [4_000, 9_600]);
        assert_eq!(timed.into_text(), " ask  not, ");

        assert!(DecodeOutput::new("ask not").word_ends().is_empty());
        assert!(
            DecodeOutput::with_word_ends("ask not", vec![4_000])
                .word_ends()
                .is_empty(),
            "too few ends are dropped"
        );
        assert!(
            DecodeOutput::with_word_ends("ask", vec![4_000, 9_600])
                .word_ends()
                .is_empty(),
            "too many ends are dropped"
        );
        assert!(
            DecodeOutput::with_word_ends("", vec![4_000])
                .word_ends()
                .is_empty(),
            "an emptied transcript drops its ends"
        );
    }

    #[test]
    fn miri_a_cancellation_flag_reaches_every_clone() {
        let flag = Arc::new(AtomicBool::new(false));
        let request = final_request().with_cancellation(Arc::clone(&flag));
        let clone = request.clone();

        let shared = clone.cancellation().expect("a clone keeps the flag");
        assert!(Arc::ptr_eq(shared, &flag), "a clone shares the flag");
        assert!(!shared.load(Ordering::Acquire));
        request
            .cancellation()
            .expect("the original keeps the flag")
            .store(true, Ordering::Release);
        assert!(shared.load(Ordering::Acquire), "a clone sees the store");

        let bare = final_request();
        assert!(bare.cancellation().is_none(), "a bare request has no flag");
        assert!(bare.clone().cancellation().is_none());
    }

    #[test]
    fn miri_sample_retirement_returns_the_owned_buffer_once() {
        let (returned, receiver) = mpsc::sync_channel(1);
        let request = DecodeRequest::new(
            DecodeMode::Final,
            vec![1.0, 2.0, 3.0],
            Vec::new(),
            String::new(),
        )
        .with_sample_retirement(move |samples| {
            returned
                .send(samples)
                .expect("the retirement receiver remains");
        });

        drop(request.clone());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(request);
        assert_eq!(
            receiver.recv().expect("the owned samples retire"),
            [1.0, 2.0, 3.0]
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
