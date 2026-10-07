//! Operations on the deterministic Realtime session fixture.

use std::future::Future;
use std::sync::Arc;

use super::{
    FixtureError, RealtimeCommitFixture, RealtimeInputSnapshotFixture, RealtimeSessionFixture,
    RealtimeTakeMetricsFixture, boxed, hour,
};
use crate::realtime::ItemResult;
use crate::take::TakeFailure;

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

    /// Returns every speech run the current take's detector has heard, as
    /// half-open 16 kHz sample ranges on the segmenter's frame grid.
    pub fn speech_runs(&self) -> Option<Vec<std::ops::Range<u64>>> {
        self.session.input().map(|input| input.take().speech_runs())
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

    /// Emits the update for final outcomes that landed since the take's last
    /// update, as the session loop does on each completion poll.
    ///
    /// # Errors
    /// Returns a revision or serialization error.
    pub fn finalized_update(&mut self) -> Result<Option<serde_json::Value>, FixtureError> {
        self.session
            .finalized_update()
            .map_err(|error| FixtureError::FinalizedUpdate(boxed(error)))?
            .map(serde_json::to_value)
            .transpose()
            .map_err(FixtureError::Serialize)
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
