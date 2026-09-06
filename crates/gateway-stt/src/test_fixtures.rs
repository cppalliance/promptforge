//! Native fixtures used only by this crate's unit tests.

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::DecodeMode;
#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

#[cfg(feature = "test-fixtures")]
use crate::realtime::{CommitReceipt, InterimEpoch, ItemResult, Session, SessionRegistry};
#[cfg(feature = "test-fixtures")]
use crate::{SpeechError, SpeechService};
#[cfg(feature = "test-fixtures")]
use gateway_stt_engine::{EnginePolicy, SttEngine};
#[cfg(feature = "test-fixtures")]
use std::future::Future;
#[cfg(feature = "test-fixtures")]
use std::sync::Arc;

/// Builds a speech service around deterministic scripted workers.
///
/// # Errors
/// Returns engine policy, startup, or worker construction failures.
#[cfg(feature = "test-fixtures")]
pub fn scripted_service(
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<SpeechService, SpeechError> {
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(window_seconds, interval_ms, gpu_available)
        .map_err(SpeechError::Engine)?;
    let engine = SttEngine::new(factory, policy).map_err(SpeechError::Engine)?;
    let final_name = engine.has_final_pass().then(|| "scripted-final".to_owned());
    let service = SpeechService::new();
    let replacement = service.scripted_replacement(engine, final_name);
    service.commit_replacement(replacement)?;
    Ok(service)
}

/// Returns every closed speech range produced by the service segmenter.
#[cfg(feature = "test-fixtures")]
#[must_use]
pub fn segment_ranges(samples: &[f32]) -> Vec<std::ops::Range<usize>> {
    let mut segmenter = crate::segment::Segmenter::new();
    let mut ranges = Vec::new();
    while let Some(range) = segmenter.poll(samples) {
        ranges.push(range);
    }
    ranges
}

/// A deterministic registry for focused Realtime session integration tests.
#[cfg(feature = "test-fixtures")]
#[derive(Clone, Debug, Default)]
pub struct RealtimeSessionRegistryFixture {
    inner: SessionRegistry,
}

#[cfg(feature = "test-fixtures")]
impl RealtimeSessionRegistryFixture {
    /// Registers one session immediately.
    ///
    /// # Errors
    /// Returns the stable capacity error when eight sessions are active or retiring.
    pub fn register(&self) -> Result<RealtimeSessionFixture, String> {
        let registration = self.inner.register().map_err(|error| error.to_string())?;
        Ok(RealtimeSessionFixture {
            session: Session::new(registration, None),
        })
    }

    /// Registers one session backed by deterministic scripted workers.
    ///
    /// # Errors
    /// Returns a stable registration, policy, or worker startup error.
    pub fn register_with_scripted_engine(
        &self,
        factory: ScriptedModelFactory,
    ) -> Result<RealtimeSessionFixture, String> {
        let registration = self.inner.register().map_err(|error| error.to_string())?;
        let policy = EnginePolicy::new(15, 500, factory.gpu_available())
            .map_err(|error| error.to_string())?;
        let engine = SttEngine::new(factory, policy).map_err(|error| error.to_string())?;
        Ok(RealtimeSessionFixture {
            session: Session::new(registration, Some(Arc::new(engine))),
        })
    }

    /// Returns active and still-retiring session ownership.
    #[must_use]
    pub fn active(&self) -> usize {
        self.inner.active()
    }

    /// Returns owned admission without polling retiring task destructors.
    #[must_use]
    pub fn owned_without_reaping(&self) -> usize {
        self.inner.owned_without_reaping()
    }
}

/// An opaque interim epoch used by the Realtime session fixture.
#[cfg(feature = "test-fixtures")]
#[derive(Clone, Copy, Debug)]
pub struct RealtimeInterimEpoch(InterimEpoch);

/// The immutable first-append configuration captured by a fixture session.
#[cfg(feature = "test-fixtures")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealtimeInputSnapshotFixture {
    item_id: String,
    prompt: String,
    include_hypothesis: bool,
}

/// The IDs established by one successful fixture commit.
#[cfg(feature = "test-fixtures")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealtimeCommitFixture(CommitReceipt);

#[cfg(feature = "test-fixtures")]
impl RealtimeCommitFixture {
    /// Returns the promoted provisional item ID.
    #[must_use]
    pub fn item_id(&self) -> &str {
        self.0.item_id()
    }

    /// Returns the preceding durable committed-item ID.
    #[must_use]
    pub fn previous_item_id(&self) -> Option<&str> {
        self.0.previous_item_id()
    }
}

#[cfg(feature = "test-fixtures")]
impl RealtimeInputSnapshotFixture {
    /// Returns the provisional item ID.
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the captured prompt.
    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// Reports whether hypothesis snapshots were negotiated.
    #[must_use]
    pub const fn include_hypothesis(&self) -> bool {
        self.include_hypothesis
    }
}

/// A deterministic Realtime session surface for focused integration tests.
#[cfg(feature = "test-fixtures")]
#[derive(Debug)]
pub struct RealtimeSessionFixture {
    session: Session,
}

#[cfg(feature = "test-fixtures")]
impl RealtimeSessionFixture {
    /// Applies one client session update.
    ///
    /// # Errors
    /// Returns the wire validation error for an invalid update.
    pub fn update_text(&mut self, text: &str) -> Result<(), String> {
        self.session
            .update_text(text)
            .map_err(|error| format!("{error:?}"))
    }

    /// Appends one Base64-encoded PCM16 chunk.
    ///
    /// # Errors
    /// Returns the audio or session ownership error.
    pub fn append_base64(&mut self, payload: &str) -> Result<(), String> {
        self.session
            .append_base64(payload)
            .map_err(|error| error.to_string())
    }

    /// Clears uncommitted input and retires its current interim task.
    ///
    /// # Errors
    /// Returns the bounded cleanup or epoch error.
    pub fn clear(&mut self) -> Result<(), String> {
        self.session.clear().map_err(|error| error.to_string())
    }

    /// Returns the current immutable input snapshot.
    pub fn input_snapshot(&self) -> Option<RealtimeInputSnapshotFixture> {
        self.session
            .input()
            .map(|input| RealtimeInputSnapshotFixture {
                item_id: input.item_id().to_owned(),
                prompt: input.snapshot().prompt().to_owned(),
                include_hypothesis: input.snapshot().include_hypothesis(),
            })
    }

    /// Returns the current input's resampled audio snapshot.
    pub fn resampled_audio(&self) -> Option<Vec<f32>> {
        self.session
            .input()
            .map(|input| input.take().uncommitted_snapshot(usize::MAX))
    }

    /// Commits the current input after reserving all item capacities.
    ///
    /// # Errors
    /// Returns validation or bounded-capacity failures without detaching input.
    pub fn commit(&mut self) -> Result<RealtimeCommitFixture, String> {
        self.session
            .commit()
            .map(RealtimeCommitFixture)
            .map_err(|error| error.to_string())
    }

    /// Records a final-segment failure before commit.
    ///
    /// # Errors
    /// Returns an error when there is no uncommitted input.
    pub fn fail_precommit(&mut self, failure: &str) -> Result<(), String> {
        self.session
            .record_pending_failure(failure.to_owned())
            .map_err(|error| error.to_string())
    }

    /// Adds one accepted nonterminal result to bounded session capacity.
    ///
    /// # Errors
    /// Returns item-state or capacity errors.
    pub fn push_delta(&mut self, item_id: &str, transcript: &str) -> Result<(), String> {
        self.session
            .push_delta(item_id, transcript.to_owned())
            .map_err(|error| error.to_string())
    }

    /// Replaces the item's newest-wins hypothesis slot.
    ///
    /// # Errors
    /// Returns item-state errors.
    pub fn replace_hypothesis(
        &mut self,
        item_id: &str,
        revision: u64,
        transcript: &str,
    ) -> Result<(), String> {
        self.session
            .replace_hypothesis(item_id, revision, transcript.to_owned())
            .map_err(|error| error.to_string())
    }

    /// Records the item's sole successful terminal outcome.
    ///
    /// # Errors
    /// Returns item-state errors, including duplicate terminal attempts.
    pub fn finalize_completed(&mut self, item_id: &str, transcript: &str) -> Result<(), String> {
        self.session
            .finalize_completed(item_id, transcript.to_owned())
            .map_err(|error| error.to_string())
    }

    /// Records the item's sole failed terminal outcome.
    ///
    /// # Errors
    /// Returns item-state errors, including duplicate terminal attempts.
    pub fn finalize_failed(&mut self, item_id: &str, message: &str) -> Result<(), String> {
        self.session
            .finalize_failed(item_id, message.to_owned())
            .map_err(|error| error.to_string())
    }

    /// Drains bounded results and releases terminal item ownership.
    pub fn drain_results(&mut self) -> Vec<serde_json::Value> {
        self.session
            .drain_results()
            .into_iter()
            .map(result_value)
            .collect()
    }

    /// Returns committed items, including terminal events awaiting drain.
    #[must_use]
    pub fn committed_count(&self) -> usize {
        self.session.committed_count()
    }

    /// Returns committed items with active accurate finalization tasks.
    #[must_use]
    pub fn finalizing_count(&self) -> usize {
        self.session.finalizing_count()
    }

    /// Returns the committed immutable prompt and take guidance.
    pub fn committed_prompt_and_guidance(&self, item_id: &str) -> Option<(String, Vec<String>)> {
        self.session
            .committed_prompt_and_guidance(item_id)
            .map(|(prompt, guidance)| (prompt.to_owned(), guidance.to_vec()))
    }

    /// Returns the sole take-owned pending precommit failure.
    pub fn pending_failure(&self) -> Option<String> {
        self.session.pending_failure()
    }

    /// Returns final segments admitted by the current take but not yet processed.
    pub fn pending_final_segments(&self) -> Option<usize> {
        self.session.pending_final_segments()
    }

    /// Awaits one item's independently owned accurate finalization.
    ///
    /// # Errors
    /// Returns item-state, task, decode, or result-mailbox errors.
    pub async fn finish_finalization(&mut self, item_id: &str) -> Result<(), String> {
        self.session
            .finish_finalization(item_id)
            .await
            .map_err(|error| error.to_string())
    }

    /// Begins an interim epoch without spawning work.
    ///
    /// # Errors
    /// Returns the session ownership or epoch error.
    pub fn begin_interim(&mut self) -> Result<RealtimeInterimEpoch, String> {
        self.session
            .begin_interim()
            .map(RealtimeInterimEpoch)
            .map_err(|error| error.to_string())
    }

    /// Spawns one session-owned interim task.
    ///
    /// # Errors
    /// Returns the bounded cleanup or epoch error.
    pub fn spawn_interim<F>(&mut self, task: F) -> Result<RealtimeInterimEpoch, String>
    where
        F: Future<Output = String> + Send + 'static,
    {
        self.session
            .spawn_interim(task)
            .map(RealtimeInterimEpoch)
            .map_err(|error| error.to_string())
    }

    /// Accepts a result and allocates its event ID only after epoch validation.
    ///
    /// # Errors
    /// Returns a serialization error if the accepted server event cannot serialize.
    pub fn accept_interim(
        &mut self,
        epoch: RealtimeInterimEpoch,
        transcript: String,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        self.session
            .accept_interim(epoch.0, transcript)
            .map(serde_json::to_value)
            .transpose()
    }

    /// Awaits and accepts the current interim task without relinquishing ownership.
    ///
    /// # Errors
    /// Returns a task, session, or serialization error.
    pub async fn finish_interim(&mut self) -> Result<Option<serde_json::Value>, String> {
        self.session
            .finish_interim()
            .await
            .map_err(|error| error.to_string())?
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| error.to_string())
    }

    /// Joins every canceled interim task without relinquishing ownership.
    ///
    /// # Errors
    /// Returns an error when a canceled task failed instead of canceling.
    pub async fn join_canceled(&mut self) -> Result<(), String> {
        self.session
            .join_canceled()
            .await
            .map_err(|error| error.to_string())
    }

    /// Returns the number of retained canceled-task joins.
    pub const fn canceled_join_count(&self) -> usize {
        self.session.canceled_join_count()
    }

    /// Returns the number of allocated server event IDs.
    pub fn allocated_event_count(&self) -> u64 {
        self.session.allocated_event_count()
    }
}

#[cfg(feature = "test-fixtures")]
fn result_value(result: ItemResult) -> serde_json::Value {
    match result {
        ItemResult::Delta {
            item_id,
            transcript,
        } => serde_json::json!({
            "type": "delta",
            "item_id": item_id,
            "transcript": transcript,
        }),
        ItemResult::Hypothesis {
            item_id,
            revision,
            transcript,
        } => serde_json::json!({
            "type": "hypothesis",
            "item_id": item_id,
            "revision": revision,
            "transcript": transcript,
        }),
        ItemResult::Completed {
            item_id,
            transcript,
            seconds,
        } => serde_json::json!({
            "type": "completed",
            "item_id": item_id,
            "transcript": transcript,
            "seconds": seconds,
        }),
        ItemResult::Failed { item_id, message } => serde_json::json!({
            "type": "failed",
            "item_id": item_id,
            "message": message,
        }),
    }
}

#[cfg(test)]
pub(crate) fn require_model() -> PathBuf {
    require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin")
}

#[cfg(test)]
pub(crate) fn jfk_samples() -> Vec<f32> {
    let path = require_fixture("PROMPTFORGE_WHISPER_AUDIO", "jfk.wav");
    let mut reader = hound::WavReader::open(path).expect("JFK fixture opens");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    assert_eq!(spec.channels, 1, "fixture must be mono");
    assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("fixture sample decodes")) / 32_768.0)
        .collect()
}

#[cfg(test)]
fn require_fixture(variable: &str, fallback: &str) -> PathBuf {
    let path = std::env::var_os(variable).map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../gateway-stt-backend-whisper/tests/fixtures")
                .join(fallback)
        },
        PathBuf::from,
    );
    assert!(
        path.is_file(),
        "native test fixture is missing: {}",
        path.display()
    );
    path
}
