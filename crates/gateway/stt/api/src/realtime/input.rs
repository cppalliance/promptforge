//! Uncommitted realtime input holding buffered audio and its sealed commit form.

use std::sync::Arc;

use gateway_stt_engine::DetectorError;

use super::wire::HypothesisInclude;
use crate::audio::{AudioBuffer, AudioError};
use crate::generation::GenerationLease;
use crate::guidance::prompt_terms;
use crate::take::{Take, TakeFailure};
const INPUT_FORMAT: &str = "audio/pcm";
const INPUT_RATE: u32 = 24_000;
const INPUT_MODEL: &str = "realtime-transcribe";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InputSnapshot {
    format: &'static str,
    rate: u32,
    model: &'static str,
    prompt: String,
    include: HypothesisInclude,
}

impl InputSnapshot {
    pub(crate) fn new(prompt: String, include: HypothesisInclude) -> Self {
        Self {
            format: INPUT_FORMAT,
            rate: INPUT_RATE,
            model: INPUT_MODEL,
            prompt,
            include,
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    #[cfg(test)]
    const fn format(&self) -> &str {
        self.format
    }

    #[cfg(test)]
    const fn rate(&self) -> u32 {
        self.rate
    }

    #[cfg(test)]
    const fn model(&self) -> &str {
        self.model
    }

    pub(crate) const fn include_hypothesis(&self) -> bool {
        !matches!(self.include, HypothesisInclude::Off)
    }

    pub(crate) const fn include_ranges(&self) -> bool {
        matches!(self.include, HypothesisInclude::Ranges)
    }
}

/// The terms every decode of a take is guided by: the generation's
/// configured vocabulary, then the session prompt's terms. The configured
/// terms lead so a glossary trimmed to fit whisper's budget keeps them.
///
/// Only tests start a take without a generation, and it has no configured
/// vocabulary.
fn guidance(engine: Option<&GenerationLease>, snapshot: &InputSnapshot) -> Vec<String> {
    let mut guidance = engine.map_or_else(Vec::new, |engine| engine.guidance().to_vec());
    guidance.extend(prompt_terms(&snapshot.prompt));
    guidance
}

#[derive(Debug)]
pub(crate) struct UncommittedInput {
    item_id: String,
    snapshot: InputSnapshot,
    audio: AudioBuffer,
    take: Take,
}

#[derive(Debug)]
pub(super) struct SealedInput {
    pub(super) item_id: String,
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) snapshot: InputSnapshot,
    pub(super) take: Take,
    pub(super) duration_seconds: f64,
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

    pub(super) fn first_append(
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
    pub(super) fn first_append_with_pcm_limit(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        payload: &str,
        limit: usize,
    ) -> Result<Self, AudioError> {
        let mut audio = AudioBuffer::default();
        audio.append_base64(payload)?;
        let guidance = guidance(engine.as_ref(), &snapshot);
        let take = Take::with_pcm_limit(guidance, engine, limit);
        Self::from_audio_and_take(item_id, snapshot, audio, take)
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) fn first_append_with_detector(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        payload: &str,
        detector: Box<dyn gateway_stt_engine::SpeechDetector>,
    ) -> Result<Self, AudioError> {
        let mut audio = AudioBuffer::default();
        audio.append_base64(payload)?;
        let take = Take::with_detector(guidance(engine.as_ref(), &snapshot), engine, detector);
        Self::from_audio_and_take(item_id, snapshot, audio, take)
    }

    /// A take whose detector does not open never starts; its failure is
    /// pending for the next append and the commit to report.
    fn from_audio(
        item_id: String,
        snapshot: InputSnapshot,
        engine: Option<GenerationLease>,
        audio: AudioBuffer,
    ) -> Result<Self, AudioError> {
        let guidance = guidance(engine.as_ref(), &snapshot);
        let take = Self::start_take(guidance.clone(), engine)
            .unwrap_or_else(|error| Take::failed(guidance, TakeFailure::Detector(error)));
        Self::from_audio_and_take(item_id, snapshot, audio, take)
    }

    /// Production sessions always hold a generation. Only tests start a
    /// take without one, which detects speech by loudness.
    fn start_take(
        guidance: Vec<String>,
        engine: Option<GenerationLease>,
    ) -> Result<Take, DetectorError> {
        match engine {
            Some(engine) => Take::new(guidance, engine),
            #[cfg(any(test, feature = "test-fixtures"))]
            None => Ok(Take::without_final(guidance)),
            #[cfg(not(any(test, feature = "test-fixtures")))]
            None => Err(DetectorError::load(
                "the session holds no speech generation",
            )),
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

    pub(super) fn append_base64(&mut self, payload: &str) -> Result<(), AudioError> {
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
    fn buffered_duration_seconds(&self) -> f64 {
        self.audio.buffered_duration_seconds()
    }

    pub(super) fn pending_failure(&self) -> Option<Arc<TakeFailure>> {
        self.take.pending_failure()
    }

    pub(super) fn record_pending_failure(&mut self, failure: TakeFailure) {
        self.take.record_failure(failure);
    }

    pub(super) fn validate_commit(&self) -> Result<(), AudioError> {
        self.audio.validate_commit()
    }

    pub(super) fn seal(self) -> Result<SealedInput, Box<(Self, AudioError)>> {
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
    use gateway_stt_engine::test_fixtures::ScriptedDetector;

    use super::{HypothesisInclude, InputSnapshot, UncommittedInput};
    use crate::test_fixtures::{ScriptedSilero, scripted_guided_generation};

    fn encoded(samples: &[i16]) -> String {
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn snapshot(prompt: &str) -> InputSnapshot {
        InputSnapshot::new(prompt.to_owned(), HypothesisInclude::Snapshots)
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

    #[test]
    fn guidance_is_the_configured_vocabulary_then_the_prompt_terms() {
        let vocabulary = vec!["WG21".to_owned(), "MCP".to_owned()];
        let (state, lease) =
            scripted_guided_generation(ScriptedSilero::new(ScriptedDetector::new([])), vocabulary);

        let prompted = UncommittedInput::new(
            "item_prompted".to_owned(),
            snapshot(" GGUF, , Lua ,"),
            Some(lease.clone()),
        );
        assert_eq!(
            prompted.take().guidance(),
            ["WG21", "MCP", "GGUF", "Lua"],
            "the configured terms lead, so a truncated glossary keeps them"
        );

        let unprompted =
            UncommittedInput::new("item_unprompted".to_owned(), snapshot(""), Some(lease));
        assert_eq!(
            unprompted.take().guidance(),
            ["WG21", "MCP"],
            "an empty prompt still carries the configured vocabulary"
        );

        drop((prompted, unprompted));
        state.shutdown();
    }
}
