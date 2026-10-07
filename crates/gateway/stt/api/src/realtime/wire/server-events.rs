//! Constructors building server events from session and item outcomes.

use super::{
    ConversationItem, DurationUsage, EffectiveSession, HypothesisRanges, InputAudioContent,
    ServerEvent, WireError,
};
use crate::realtime::result_mailbox::{ItemFailure, ItemResult};
use crate::realtime::wire::vocabulary::{OptionalNullable, RequiredNullable};
use crate::take::InterimSnapshot;
impl ServerEvent {
    pub(in crate::realtime) fn session_created(
        event_id: String,
        session: EffectiveSession,
    ) -> Self {
        Self::SessionCreated { event_id, session }
    }

    pub(in crate::realtime) fn session_updated(
        event_id: String,
        session: EffectiveSession,
    ) -> Self {
        Self::SessionUpdated { event_id, session }
    }

    pub(in crate::realtime) fn input_cleared(event_id: String) -> Self {
        Self::InputCleared { event_id }
    }

    pub(in crate::realtime) fn hypothesis(
        event_id: String,
        item_id: String,
        revision: u64,
        snapshot: InterimSnapshot,
        audio_start_ms: u64,
        audio_end_ms: u64,
        ranges: Option<HypothesisRanges>,
    ) -> Self {
        let (transcript, finalized, agreed, tentative) = snapshot.into_parts();
        Self::TranscriptionHypothesis {
            event_id,
            item_id,
            content_index: 0,
            revision,
            transcript,
            finalized,
            agreed,
            tentative,
            audio_start_ms,
            audio_end_ms,
            finalized_through_ms: ranges.map(|ranges| ranges.finalized_through_ms),
            finalized_seq: ranges.map(|ranges| ranges.finalized_seq),
        }
    }

    pub(in crate::realtime) fn committed(
        committed_event_id: String,
        created_event_id: String,
        item_id: String,
        previous_item_id: Option<String>,
    ) -> [Self; 2] {
        let previous = previous_item_id.map_or(RequiredNullable::Null, RequiredNullable::Value);
        [
            Self::InputCommitted {
                event_id: committed_event_id,
                item_id: item_id.clone(),
                previous_item_id: previous.clone(),
            },
            Self::ItemCreated {
                event_id: created_event_id,
                previous_item_id: previous,
                item: ConversationItem {
                    id: item_id,
                    r#type: "message".to_owned(),
                    status: "completed".to_owned(),
                    role: "user".to_owned(),
                    content: vec![InputAudioContent {
                        r#type: "input_audio".to_owned(),
                        transcript: RequiredNullable::Null,
                    }],
                },
            },
        ]
    }

    pub(in crate::realtime) fn item_result(event_id: String, result: ItemResult) -> Self {
        match result {
            ItemResult::Completed {
                item_id,
                transcript,
                seconds,
            } => Self::TranscriptionCompleted {
                event_id,
                item_id,
                content_index: 0,
                transcript,
                usage: DurationUsage {
                    r#type: "duration".to_owned(),
                    seconds,
                },
            },
            ItemResult::Failed { item_id, failure } => Self::TranscriptionFailed {
                event_id,
                item_id,
                content_index: 0,
                error: item_failure_error(&failure),
            },
        }
    }
}

fn item_failure_error(failure: &ItemFailure) -> WireError {
    let (kind, code, message, param) = match failure {
        ItemFailure::PrecommitTranscriptionFailed(_) => (
            "server_error",
            "precommit_transcription_failed",
            "Accurate precommit transcription failed",
            OptionalNullable::Null,
        ),
        ItemFailure::TranscriptionFailed(_) => (
            "server_error",
            "transcription_failed",
            "Authoritative transcription failed",
            OptionalNullable::Value("audio".to_owned()),
        ),
    };
    WireError {
        r#type: kind.to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
        param,
        event_id: OptionalNullable::Missing,
    }
}
