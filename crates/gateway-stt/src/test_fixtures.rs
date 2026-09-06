//! Native fixtures used only by this crate's unit tests.

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::DecodeMode;
#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

#[cfg(feature = "test-fixtures")]
use crate::SttRuntime;
#[cfg(feature = "test-fixtures")]
use crate::realtime::{InterimEpoch, Session, SessionRegistry};
#[cfg(feature = "test-fixtures")]
use gateway_stt_engine::{EnginePolicy, SttEngine, TranscribeError};
#[cfg(feature = "test-fixtures")]
use std::future::Future;

/// Builds a speech runtime around deterministic scripted workers.
///
/// # Errors
/// Returns engine policy, startup, or worker construction failures.
#[cfg(feature = "test-fixtures")]
pub fn scripted_runtime(
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<SttRuntime, TranscribeError> {
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(window_seconds, interval_ms, gpu_available)?;
    let engine = SttEngine::new(factory, policy)?;
    let final_name = engine.has_final_pass().then(|| "scripted-final".to_owned());
    Ok(SttRuntime::from_scripted_engine(
        engine,
        "scripted-interim".to_owned(),
        final_name,
        Vec::new(),
    ))
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

    /// Returns active and still-retiring session ownership.
    #[must_use]
    pub fn active(&self) -> usize {
        self.inner.active()
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
