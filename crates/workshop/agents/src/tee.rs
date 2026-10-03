//! The recorder a conversation's run writes through: every call goes on
//! to the inner recorder, and what the inner recorder accepted also
//! reaches the conversation.
//!
//! The tee names the conversation's agent in the run's metadata, notes
//! the run the inner recorder begins, and hands each event record the
//! inner recorder accepted to the conversation, so the record, the live
//! broadcast, and the transcript agree event for event. A refused write
//! is returned as the inner recorder's own, and its event reaches no one.

use std::sync::Arc;

use harness::record::{
    Record, RecordKind, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use promptforge::event::Event;

use crate::conversation::Conversation;

/// A [`RunRecorder`] over an inner recorder that feeds one conversation.
pub(crate) struct ConversationRecorder {
    conversation: Conversation,
    inner: Arc<dyn RunRecorder>,
}

impl ConversationRecorder {
    /// The tee over `inner` feeding `conversation`.
    pub(crate) fn new(conversation: Conversation, inner: Arc<dyn RunRecorder>) -> Self {
        Self {
            conversation,
            inner,
        }
    }
}

impl RunRecorder for ConversationRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let meta = RunMeta {
                agent: self.conversation.agent().to_owned(),
                ..meta
            };
            let run = self.inner.begin_run(meta).await?;
            self.conversation.note_run(run);
            Ok(run)
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let event = (record.kind == RecordKind::Event).then(|| record.payload.clone());
            self.inner.append(run, record).await?;
            if let Some(payload) = event {
                // The run serialized this event for the record a moment
                // ago, so it reads back; a payload that does not is
                // logged and left out of the transcript.
                match serde_json::from_value::<Event>(payload) {
                    Ok(event) => self.conversation.observe(&event),
                    Err(error) => tracing::error!(
                        conversation = %self.conversation.id(),
                        %error,
                        "a recorded event did not read back; the transcript skips it"
                    ),
                }
            }
            Ok(())
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        self.inner.end_run(run, outcome)
    }
}

#[cfg(test)]
#[path = "tee-tests.rs"]
mod tests;
