//! A Whisper model factory that records every decode it runs.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use gateway_stt_backend_whisper::WhisperModelFactory;
use gateway_stt_engine::{
    DecodeMode, DecodeOutput, DecodeRequest, Decoder, ModelFactory, TranscribeError,
};

#[derive(Debug)]
pub(super) struct Decode {
    pub(super) mode: DecodeMode,
    pub(super) samples: u64,
    pub(super) text: String,
    pub(super) wall: Duration,
}

pub(super) type Decodes = Arc<Mutex<Vec<Decode>>>;

/// Removes and returns every decode recorded so far.
pub(super) fn take_decodes(decodes: &Decodes) -> Vec<Decode> {
    std::mem::take(&mut *decodes.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Wraps the Whisper factory so every decode records its raw output.
#[derive(Debug)]
pub(super) struct RecordingFactory {
    pub(super) inner: WhisperModelFactory,
    pub(super) decodes: Decodes,
}

impl ModelFactory for RecordingFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        let decodes = Arc::clone(&self.decodes);
        Ok(self
            .inner
            .create(mode)?
            .map(|inner| Box::new(RecordingDecoder { inner, decodes }) as Box<dyn Decoder>))
    }
}

struct RecordingDecoder {
    inner: Box<dyn Decoder>,
    decodes: Decodes,
}

impl Decoder for RecordingDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<DecodeOutput, TranscribeError> {
        let mode = request.mode();
        let samples = u64::try_from(request.samples().len()).unwrap_or(u64::MAX);
        let started = Instant::now();
        let output = self.inner.decode(request)?;
        let wall = started.elapsed();
        self.decodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Decode {
                mode,
                samples,
                text: output.text().to_owned(),
                wall,
            });
        Ok(output)
    }
}
