//! The recorder a session's runs write through: every call goes on to the
//! Host's recorder, and what the Host's recorder took also reaches the
//! session.
//!
//! The tee names the session's agent in each run's metadata, notes each
//! run the Host's recorder begins, and hands each event record the Host's
//! recorder accepted to the session core, so the recorder, the live
//! broadcast, and the transcript agree event for event. A run that ends
//! before the loop sees it, a parse failure or a refusal, reaches the
//! transcript the same way, since its parse events are records too.

use std::sync::Arc;

use harness_runner::recorder::{
    Record, RecordKind, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use promptforge::event::Event;

use super::SessionCore;

/// A [`RunRecorder`] over the Host's recorder that feeds one session.
pub(crate) struct SessionRecorder {
    core: Arc<SessionCore>,
}

impl SessionRecorder {
    /// The tee over `core`'s Host recorder, feeding `core`.
    pub(crate) fn new(core: Arc<SessionCore>) -> Self {
        Self { core }
    }
}

impl RunRecorder for SessionRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let meta = RunMeta {
                agent: self.core.agent.clone(),
                ..meta
            };
            let run = self.core.recorder.begin_run(meta).await?;
            self.core.record_run(run);
            Ok(run)
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let event = (record.kind == RecordKind::Event).then(|| record.payload.clone());
            self.core.recorder.append(run, record).await?;
            if let Some(payload) = event {
                // The run serialized this event for the record a moment
                // ago, so it reads back; a payload that does not is
                // logged and left out of the transcript.
                match serde_json::from_value::<Event>(payload) {
                    Ok(event) => self.core.observe(&event),
                    Err(error) => tracing::error!(
                        session = %self.core.id,
                        %error,
                        "a recorded event did not read back; the transcript skips it"
                    ),
                }
            }
            Ok(())
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        self.core.recorder.end_run(run, outcome)
    }
}
