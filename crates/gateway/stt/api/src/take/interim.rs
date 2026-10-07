//! The audio window an interim decode reads, and the interim transcript
//! snapshot split into finalized, agreed, and tentative parts with the
//! finalized range behind its finalized part.

use gateway_stt_engine::EnginePolicy;

use super::pcm::RetainedPcm;
use super::{Take, TakeState};
use crate::audio::AudioError;

/// Silence an interim window keeps after the last speech, 300 ms: long
/// enough for whisper to end a sentence with its punctuation.
pub(crate) const SPEECH_TAIL_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE * 3 / 10) as u64;

#[derive(Debug)]
pub(crate) struct InterimAudioWindow {
    pub(crate) samples: RetainedPcm,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) segment_start: u64,
}

impl Take {
    /// The open segment's audio up to the earlier of the buffer end and the
    /// speech tail, at most `window_samples` long. Before any speech, or once
    /// the segment starts past the speech tail, the window is empty at the
    /// segment start.
    pub(crate) fn interim_window(
        &self,
        window_samples: usize,
    ) -> Result<InterimAudioWindow, AudioError> {
        let (segment_start, speech_end) = {
            let segmenter = TakeState::lock(&self.state.segmenter);
            (segmenter.consumed(), segmenter.speech_end())
        };
        let tail_end = speech_end.map_or(0, |end| end.saturating_add(SPEECH_TAIL_SAMPLES));
        let buffer = TakeState::lock(&self.state.buffer);
        let end = buffer.end().min(tail_end).max(segment_start);
        let start = segment_start
            .max(end.saturating_sub(u64::try_from(window_samples).unwrap_or(u64::MAX)));
        Ok(InterimAudioWindow {
            samples: buffer.copy_range(start..end)?,
            start,
            end,
            segment_start,
        })
    }
}

/// Absolute sample watermark of finalized text and the count of final
/// outcomes applied to the take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FinalizedRange {
    pub(crate) through_samples: u64,
    pub(crate) seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterimSnapshot {
    transcript: String,
    finalized: String,
    agreed: String,
    tentative: String,
}

impl InterimSnapshot {
    pub(super) fn new(finalized: String, agreed: String, tentative: String) -> Self {
        let transcript = format!("{finalized}{agreed}{tentative}");
        Self {
            transcript,
            finalized,
            agreed,
            tentative,
        }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.transcript.is_empty()
    }

    pub(crate) fn into_parts(self) -> (String, String, String, String) {
        (self.transcript, self.finalized, self.agreed, self.tentative)
    }
}

#[cfg(test)]
#[path = "interim-tests.rs"]
mod tests;
