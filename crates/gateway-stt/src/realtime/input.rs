use crate::audio::{AudioBuffer, AudioError};
use crate::generation::GenerationLease;
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

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    #[cfg(test)]
    pub(crate) const fn format(&self) -> &str {
        self.format
    }

    #[cfg(test)]
    pub(crate) const fn rate(&self) -> u32 {
        self.rate
    }

    #[cfg(test)]
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

#[derive(Debug)]
pub(crate) struct SealedInput {
    pub(crate) item_id: String,
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) snapshot: InputSnapshot,
    pub(crate) take: Take,
    pub(crate) duration_seconds: f64,
}
impl UncommittedInput {
    #[cfg(test)]
    pub(crate) fn new(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
    ) -> Self {
        Self::from_audio(item_id, snapshot, engine, AudioBuffer::default())
            .unwrap_or_else(|_| unreachable!("an empty audio buffer owns no retained PCM"))
    }

    pub(crate) fn first_append(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        payload: &str,
    ) -> Result<Self, AudioError> {
        let mut audio = AudioBuffer::default();
        audio.append_base64(payload)?;
        Self::from_audio(item_id, snapshot, engine, audio)
    }

    #[cfg(test)]
    pub(crate) fn first_append_with_pcm_limit(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        payload: &str,
        limit: usize,
    ) -> Result<Self, AudioError> {
        let mut audio = AudioBuffer::default();
        audio.append_base64(payload)?;
        let guidance = Self::guidance(&snapshot);
        let take = Take::with_pcm_limit(guidance, engine, limit);
        Self::from_audio_and_take(item_id, snapshot, audio, take)
    }

    fn from_audio(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        audio: AudioBuffer,
    ) -> Result<Self, AudioError> {
        let take = Take::new(Self::guidance(&snapshot), engine);
        Self::from_audio_and_take(item_id, snapshot, audio, take)
    }

    fn guidance(snapshot: &InputSnapshot) -> Vec<String> {
        if snapshot.prompt.is_empty() {
            Vec::new()
        } else {
            vec![snapshot.prompt.clone()]
        }
    }

    fn from_audio_and_take(
        item_id: String,
        snapshot: InputSnapshot,
        mut audio: AudioBuffer,
        take: Take,
    ) -> Result<Self, AudioError> {
        take.append(audio.take_resampled())?;
        take.submit_closed_segments();
        Ok(Self {
            item_id,
            snapshot,
            audio,
            take,
        })
    }

    pub(crate) fn append_base64(&mut self, payload: &str) -> Result<(), AudioError> {
        let mut audio = self.audio.clone();
        audio.append_base64(payload)?;
        self.take.append(audio.take_resampled())?;
        self.audio = audio;
        self.take.submit_closed_segments();
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

    #[cfg(feature = "test-fixtures")]
    pub(crate) const fn input_samples(&self) -> u64 {
        self.audio.input_samples
    }

    #[cfg(test)]
    pub(crate) fn buffered_duration_seconds(&self) -> f64 {
        self.audio.buffered_duration_seconds()
    }

    pub(crate) fn pending_failure(&self) -> Option<String> {
        self.take.pending_failure()
    }

    pub(crate) fn record_pending_failure(&mut self, failure: String) {
        self.take.record_failure(failure);
    }

    pub(crate) fn validate_commit(&self) -> Result<(), AudioError> {
        self.audio.validate_commit()
    }

    pub(crate) fn seal(self) -> Result<SealedInput, Box<(Self, AudioError)>> {
        let mut audio = self.audio.clone();
        let committed = audio.commit_validated();
        let duration_seconds = committed.duration_seconds();
        if let Err(error) = self.take.append(committed.into_samples()) {
            return Err(Box::new((self, error)));
        }
        Ok(SealedInput {
            item_id: self.item_id,
            #[cfg(any(test, feature = "test-fixtures"))]
            snapshot: self.snapshot,
            take: self.take,
            duration_seconds,
        })
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
