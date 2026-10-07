//! Deterministic fixtures split by service responsibility.

#[cfg(feature = "test-fixtures")]
use std::future::Future;

#[cfg(feature = "test-fixtures")]
use crate::SpeechError;
#[cfg(feature = "test-fixtures")]
use crate::realtime::{CommitReceipt, Session, SessionRegistry};

#[cfg(feature = "test-fixtures")]
mod generation;
#[cfg(feature = "test-fixtures")]
mod hour;
#[cfg(feature = "test-fixtures")]
pub mod native;
#[cfg(feature = "test-fixtures")]
mod replay;
#[cfg(feature = "test-fixtures")]
mod segment;
#[cfg(feature = "test-fixtures")]
mod session;

#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::{DecodeMode, Decoder, EnginePolicy, ModelFactory, TranscribeError};
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
pub use replay::{
    ReplayError, ReplayFinal, ReplayOutcome, ReplayScript, ReplaySnapshot, ReplayTake, ReplayTick,
};
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
