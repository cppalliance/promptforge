use std::sync::Arc;

use gateway_stt_engine::SttEngine;

use crate::audio::{AudioBuffer, AudioError};
use crate::take::Take;

const INPUT_FORMAT: &str = "audio/pcm";
const INPUT_RATE: u32 = 24_000;
const INPUT_MODEL: &str = "realtime-transcribe";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InputSnapshot {
    format: &'static str,
    rate: u32,
    model: &'static str,
    prompt: String,
    include_hypothesis: bool,
}

impl InputSnapshot {
    pub(crate) fn new(prompt: String, include_hypothesis: bool) -> Self {
        Self {
            format: INPUT_FORMAT,
            rate: INPUT_RATE,
            model: INPUT_MODEL,
            prompt,
            include_hypothesis,
        }
    }

    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    pub(crate) const fn format(&self) -> &str {
        self.format
    }

    pub(crate) const fn rate(&self) -> u32 {
        self.rate
    }

    pub(crate) const fn model(&self) -> &str {
        self.model
    }

    pub(crate) const fn include_hypothesis(&self) -> bool {
        self.include_hypothesis
    }
}

#[derive(Debug)]
pub(crate) struct UncommittedInput {
    item_id: String,
    snapshot: InputSnapshot,
    audio: AudioBuffer,
    take: Take,
}

impl UncommittedInput {
    pub(crate) fn new(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<Arc<SttEngine>>,
    ) -> Self {
        Self::from_audio(item_id, snapshot, engine, AudioBuffer::default())
    }

    pub(crate) fn first_append(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<Arc<SttEngine>>,
        payload: &str,
    ) -> Result<Self, AudioError> {
        let mut audio = AudioBuffer::default();
        audio.append_base64(payload)?;
        Ok(Self::from_audio(item_id, snapshot, engine, audio))
    }

    fn from_audio(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<Arc<SttEngine>>,
        mut audio: AudioBuffer,
    ) -> Self {
        let guidance = if snapshot.prompt.is_empty() {
            Vec::new()
        } else {
            vec![snapshot.prompt.clone()]
        };
        let take = Take::new(guidance, engine);
        take.append(&audio.take_resampled());
        Self {
            item_id,
            snapshot,
            audio,
            take,
        }
    }

    pub(crate) fn append_base64(&mut self, payload: &str) -> Result<(), AudioError> {
        self.audio.append_base64(payload)?;
        self.take.append(&self.audio.take_resampled());
        Ok(())
    }

    pub(crate) fn item_id(&self) -> &str {
        &self.item_id
    }

    pub(crate) const fn snapshot(&self) -> &InputSnapshot {
        &self.snapshot
    }

    pub(crate) const fn take(&self) -> &Take {
        &self.take
    }

    pub(crate) fn buffered_duration_seconds(&self) -> f64 {
        self.audio.buffered_duration_seconds()
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::{InputSnapshot, UncommittedInput};

    fn encoded(samples: &[i16]) -> String {
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn snapshot(prompt: &str) -> InputSnapshot {
        InputSnapshot::new(prompt.to_owned(), true)
    }

    #[test]
    fn miri_input_owns_snapshot_audio_resampler_and_take_state() {
        let mut input = UncommittedInput::new("item_one".to_owned(), snapshot("first"), None);
        input
            .append_base64(&encoded(&vec![512; 2_400]))
            .expect("audio appends");

        assert_eq!(input.snapshot().prompt(), "first");
        assert_eq!(input.snapshot().format(), "audio/pcm");
        assert_eq!(input.snapshot().rate(), 24_000);
        assert_eq!(input.snapshot().model(), "realtime-transcribe");
        assert!(input.snapshot().include_hypothesis());
        assert_eq!(input.item_id(), "item_one");
        assert!((input.buffered_duration_seconds() - 0.1).abs() < f64::EPSILON);
        assert_eq!(input.take().guidance(), ["first"]);
        assert!(!input.take().uncommitted_snapshot(usize::MAX).is_empty());
    }
}
