//! Deterministic fixtures split by service responsibility.

#[cfg(feature = "test-fixtures")]
use std::future::Future;
#[cfg(feature = "test-fixtures")]
use std::sync::Arc;

#[cfg(feature = "test-fixtures")]
use crate::SpeechError;
#[cfg(feature = "test-fixtures")]
use crate::realtime::{CommitReceipt, ItemResult, Session, SessionRegistry};
#[cfg(feature = "test-fixtures")]
use crate::take::TakeFailure;

#[cfg(feature = "test-fixtures")]
mod generation;
#[cfg(feature = "test-fixtures")]
mod hour;
#[cfg(all(test, not(miri)))]
mod native;
#[cfg(feature = "test-fixtures")]
mod segment;

#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::DecodeMode;
#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
#[cfg(feature = "test-fixtures")]
pub use generation::{
    GenerationOwnershipFixture, GenerationWorkerJobFixture, generation_counts,
    generation_ownership, load_scripted_initial, load_scripted_initial_with_cancellation,
    scripted_service,
};
#[cfg(feature = "test-fixtures")]
pub use hour::{
    HourSimulationProbe, RealtimeTakeMetricsFixture, hour_marker_input, hour_simulation_service,
};
#[cfg(all(test, not(miri)))]
pub(crate) use native::{jfk_samples, require_model};
#[cfg(feature = "test-fixtures")]
pub use segment::segment_ranges;

/// A boxed crate-internal source surfaced through [`FixtureError`].
#[cfg(feature = "test-fixtures")]
type BoxedSource = Box<dyn std::error::Error + Send + Sync + 'static>;

#[cfg(feature = "test-fixtures")]
fn boxed(source: impl std::error::Error + Send + Sync + 'static) -> BoxedSource {
    Box::new(source)
}

/// One deterministic fixture operation failure.
#[cfg(feature = "test-fixtures")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FixtureError {
    /// Session registration was rejected.
    #[error("register fixture session")]
    #[non_exhaustive]
    Register(#[source] BoxedSource),
    /// The scripted service failed to start.
    #[error("start scripted service")]
    #[non_exhaustive]
    ScriptedService(#[source] SpeechError),
    /// The scripted generation never published an engine.
    #[error("scripted generation did not publish")]
    ScriptedGenerationNotPublished,
    /// The hour simulation service failed to start.
    #[error("start hour simulation service")]
    #[non_exhaustive]
    HourSimulationService(#[source] SpeechError),
    /// The hour simulation generation never published an engine.
    #[error("hour simulation generation did not publish")]
    HourSimulationNotPublished,
    /// The session update was rejected.
    #[error("apply session update")]
    #[non_exhaustive]
    SessionUpdate(#[source] BoxedSource),
    /// The audio append failed.
    #[error("append fixture audio")]
    #[non_exhaustive]
    Append(#[source] BoxedSource),
    /// The input clear failed.
    #[error("clear fixture input")]
    #[non_exhaustive]
    Clear(#[source] BoxedSource),
    /// The commit failed.
    #[error("commit fixture input")]
    #[non_exhaustive]
    Commit(#[source] BoxedSource),
    /// Recording the precommit failure failed.
    #[error("record precommit failure")]
    #[non_exhaustive]
    RecordPrecommitFailure(#[source] BoxedSource),
    /// The delta push failed.
    #[error("push fixture delta")]
    #[non_exhaustive]
    PushDelta(#[source] BoxedSource),
    /// The hypothesis replacement failed.
    #[error("replace fixture hypothesis")]
    #[non_exhaustive]
    ReplaceHypothesis(#[source] BoxedSource),
    /// The completed terminal outcome was rejected.
    #[error("finalize fixture item completed")]
    #[non_exhaustive]
    FinalizeCompleted(#[source] BoxedSource),
    /// The failed terminal outcome was rejected.
    #[error("finalize fixture item failed")]
    #[non_exhaustive]
    FinalizeFailed(#[source] BoxedSource),
    /// The finalization replacement failed.
    #[error("replace fixture finalization")]
    #[non_exhaustive]
    ReplaceFinalization(#[source] BoxedSource),
    /// The finalization join failed.
    #[error("finish fixture finalization")]
    #[non_exhaustive]
    FinishFinalization(#[source] BoxedSource),
    /// The interim task spawn failed.
    #[error("spawn fixture interim")]
    #[non_exhaustive]
    SpawnInterim(#[source] BoxedSource),
    /// The interim decode scheduling failed.
    #[error("schedule fixture interim")]
    #[non_exhaustive]
    ScheduleInterim(#[source] BoxedSource),
    /// The interim accept failed.
    #[error("accept fixture interim")]
    #[non_exhaustive]
    AcceptInterim(#[source] BoxedSource),
    /// The interim join failed.
    #[error("finish fixture interim")]
    #[non_exhaustive]
    FinishInterim(#[source] BoxedSource),
    /// The canceled-task join failed.
    #[error("join canceled fixture interims")]
    #[non_exhaustive]
    JoinCanceled(#[source] BoxedSource),
    /// Result serialization failed.
    #[error("serialize fixture result")]
    #[non_exhaustive]
    Serialize(#[source] serde_json::Error),
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
    pub fn register(&self) -> Result<RealtimeSessionFixture, FixtureError> {
        let registration = self
            .inner
            .register()
            .map_err(|error| FixtureError::Register(boxed(error)))?;
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
    ) -> Result<RealtimeSessionFixture, FixtureError> {
        let registration = self
            .inner
            .register()
            .map_err(|error| FixtureError::Register(boxed(error)))?;
        let service = scripted_service(factory, 15, 500).map_err(FixtureError::ScriptedService)?;
        let engine = service
            .state
            .active()
            .ok_or(FixtureError::ScriptedGenerationNotPublished)?;
        Ok(RealtimeSessionFixture {
            session: Session::new(registration, Some(engine)),
        })
    }

    /// Registers one session backed by bounded hour-equivalent decoders.
    ///
    /// # Errors
    /// Returns a stable registration, policy, or worker startup error.
    pub fn register_with_hour_simulation(
        &self,
        probe: &HourSimulationProbe,
    ) -> Result<RealtimeSessionFixture, FixtureError> {
        let registration = self
            .inner
            .register()
            .map_err(|error| FixtureError::Register(boxed(error)))?;
        let service = hour::hour_simulation_service(probe.clone())
            .map_err(FixtureError::HourSimulationService)?;
        let engine = service
            .state
            .active()
            .ok_or(FixtureError::HourSimulationNotPublished)?;
        Ok(RealtimeSessionFixture {
            session: Session::new(registration, Some(engine)),
        })
    }

    /// Returns active and still-retiring session ownership.
    #[must_use]
    pub fn active(&self) -> usize {
        self.inner.active()
    }

    /// Returns the number of registry cleanup events emitted.
    #[must_use]
    pub fn cleanup_event_count(&self) -> usize {
        self.inner.cleanup_event_count()
    }

    /// Returns the number of retired tasks whose joins failed after cancellation.
    #[must_use]
    pub fn retired_task_failures(&self) -> usize {
        self.inner.retired_task_failures()
    }

    /// Waits for the next registry-owned session cleanup.
    pub fn cleanup_notified(&self) -> impl Future<Output = ()> + '_ {
        self.inner.cleanup_notified()
    }
}

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
    pub fn update_text(&mut self, text: &str) -> Result<(), FixtureError> {
        self.session
            .update_text(text)
            .map_err(|error| FixtureError::SessionUpdate(boxed(error)))
    }

    /// Appends one Base64-encoded PCM16 chunk.
    ///
    /// # Errors
    /// Returns the audio or session ownership error.
    pub fn append_base64(&mut self, payload: &str) -> Result<(), FixtureError> {
        self.session
            .append_base64(payload)
            .map_err(|error| FixtureError::Append(boxed(error)))
    }

    /// Clears uncommitted input and retires its current interim task.
    ///
    /// # Errors
    /// Returns the bounded cleanup or epoch error.
    pub fn clear(&mut self) -> Result<(), FixtureError> {
        self.session
            .clear()
            .map_err(|error| FixtureError::Clear(boxed(error)))
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

    /// Returns one bounded ownership snapshot for the active production take.
    pub fn take_metrics(&self) -> Option<RealtimeTakeMetricsFixture> {
        self.session.input().map(hour::take_metrics)
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
    pub fn commit(&mut self) -> Result<RealtimeCommitFixture, FixtureError> {
        self.session
            .commit()
            .map(RealtimeCommitFixture)
            .map_err(|error| FixtureError::Commit(boxed(error)))
    }

    /// Records a final-segment failure before commit.
    ///
    /// # Errors
    /// Returns an error when there is no uncommitted input.
    pub fn fail_precommit(&mut self, failure: &str) -> Result<(), FixtureError> {
        self.session
            .record_pending_failure(TakeFailure::Recorded(failure.to_owned()))
            .map_err(|error| FixtureError::RecordPrecommitFailure(boxed(error)))
    }

    /// Adds one accepted nonterminal result to bounded session capacity.
    ///
    /// # Errors
    /// Returns item-state or capacity errors.
    pub fn push_delta(&mut self, item_id: &str, transcript: &str) -> Result<(), FixtureError> {
        self.session
            .push_delta(item_id, transcript.to_owned())
            .map_err(|error| FixtureError::PushDelta(boxed(error)))
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
    ) -> Result<(), FixtureError> {
        self.session
            .replace_hypothesis(item_id, revision, transcript.to_owned())
            .map_err(|error| FixtureError::ReplaceHypothesis(boxed(error)))
    }

    /// Records the item's sole successful terminal outcome.
    ///
    /// # Errors
    /// Returns item-state errors, including duplicate terminal attempts.
    pub fn finalize_completed(
        &mut self,
        item_id: &str,
        transcript: &str,
    ) -> Result<(), FixtureError> {
        self.session
            .finalize_completed(item_id, transcript.to_owned())
            .map_err(|error| FixtureError::FinalizeCompleted(boxed(error)))
    }

    /// Records the item's sole failed terminal outcome.
    ///
    /// # Errors
    /// Returns item-state errors, including duplicate terminal attempts.
    pub fn finalize_failed(&mut self, item_id: &str, message: &str) -> Result<(), FixtureError> {
        self.session
            .finalize_failed(item_id, message.to_owned())
            .map_err(|error| FixtureError::FinalizeFailed(boxed(error)))
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

    /// Replaces one committed item's accurate finalization with controlled work.
    ///
    /// # Errors
    /// Returns an error when the committed item does not exist.
    pub fn replace_finalization<F>(&mut self, item_id: &str, task: F) -> Result<(), FixtureError>
    where
        F: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        self.session
            .replace_finalization(item_id, async move {
                task.await
                    .map_err(|error| Arc::new(TakeFailure::Recorded(error.to_string())))
            })
            .map_err(|error| FixtureError::ReplaceFinalization(boxed(error)))
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
    pub async fn finish_finalization(&mut self, item_id: &str) -> Result<(), FixtureError> {
        self.session
            .finish_finalization(item_id)
            .await
            .map_err(|error| FixtureError::FinishFinalization(boxed(error)))
    }

    /// Spawns one session-owned interim task.
    ///
    /// # Errors
    /// Returns the bounded cleanup or epoch error.
    pub fn spawn_interim<F>(&mut self, task: F) -> Result<(), FixtureError>
    where
        F: Future<Output = String> + Send + 'static,
    {
        self.session
            .spawn_interim(task)
            .map(|_| ())
            .map_err(|error| FixtureError::SpawnInterim(boxed(error)))
    }

    /// Accepts one interim, clears its input, then rejects the stale epoch.
    ///
    /// # Errors
    /// Returns an ownership, cleanup, epoch, or serialization error.
    pub fn accept_interim_across_clear(
        &mut self,
        current: &str,
        stale: &str,
    ) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), FixtureError> {
        let epoch = self
            .session
            .begin_interim()
            .map_err(|error| FixtureError::AcceptInterim(boxed(error)))?;
        let current = self
            .session
            .accept_interim(epoch, current.to_owned())
            .map(serde_json::to_value)
            .transpose()
            .map_err(FixtureError::Serialize)?;
        self.session
            .clear()
            .map_err(|error| FixtureError::Clear(boxed(error)))?;
        let stale = self
            .session
            .accept_interim(epoch, stale.to_owned())
            .map(serde_json::to_value)
            .transpose()
            .map_err(FixtureError::Serialize)?;
        Ok((current, stale))
    }

    /// Awaits and accepts the current interim task without relinquishing ownership.
    ///
    /// # Errors
    /// Returns a task, session, or serialization error.
    pub async fn finish_interim(&mut self) -> Result<Option<serde_json::Value>, FixtureError> {
        self.session
            .finish_interim()
            .await
            .map_err(|error| FixtureError::FinishInterim(boxed(error)))?
            .map(serde_json::to_value)
            .transpose()
            .map_err(FixtureError::Serialize)
    }

    /// Schedules and accepts one production interim decode.
    ///
    /// # Errors
    /// Returns an audio, generation, task, session, or serialization error.
    pub async fn run_interim(&mut self) -> Result<Option<serde_json::Value>, FixtureError> {
        self.session
            .schedule_interim()
            .map_err(|error| FixtureError::ScheduleInterim(boxed(error)))?;
        self.finish_interim().await
    }

    /// Joins every canceled interim task without relinquishing ownership.
    ///
    /// # Errors
    /// Returns an error when a canceled task failed instead of canceling.
    pub async fn join_canceled(&mut self) -> Result<(), FixtureError> {
        self.session
            .join_canceled()
            .await
            .map_err(|error| FixtureError::JoinCanceled(boxed(error)))
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
        ItemResult::Failed { item_id, failure } => serde_json::json!({
            "type": "failed",
            "item_id": item_id,
            "message": failure.diagnostic(),
        }),
    }
}
