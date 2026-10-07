//! Session state struct, error type, and interim task definitions.

use crate::audio::AudioError;
use crate::generation::GenerationLease;
use crate::realtime::input::UncommittedInput;
use crate::realtime::item::{CommittedItem, FinalizationError};
use crate::realtime::registry::SessionRegistration;
use crate::realtime::result_mailbox::{MailboxError, ResultMailbox};
use crate::realtime::wire::{EffectiveSession, HypothesisRanges, IdGenerator};
use crate::take::{InterimSnapshot, TakeFailure};
use gateway_stt_engine::{DecodeOutput, TranscribeError};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
pub(super) const SESSION_CANCEL_JOIN_CAPACITY: usize = 8;
pub(super) const MAX_COMMITTED_ITEMS_PER_SESSION: usize = 4;
type InterimTask = JoinHandle<InterimTaskOutput>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InterimEpoch(pub(super) u64);
#[derive(Debug)]
pub(super) enum InterimTaskOutput {
    #[cfg(any(test, feature = "test-fixtures"))]
    Fixture(InterimEpoch, String),
    Decode {
        epoch: InterimEpoch,
        item_id: String,
        segment_start: u64,
        audio_start: u64,
        audio_end: u64,
        transcript: Result<DecodeOutput, TranscribeError>,
    },
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error("the canceled interim task join capacity is reached")]
    CancelJoinAtCapacity,
    #[error("the interim epoch space is exhausted")]
    EpochExhausted,
    #[error("a canceled interim task failed while joining")]
    CanceledTaskFailed,
    #[error("there is no uncommitted input")]
    NoInput,
    #[error("the committed realtime item limit is reached")]
    CommittedItemsAtCapacity,
    #[error("speech generation is unavailable")]
    GenerationUnavailable,
    #[error("transcription failed")]
    #[non_exhaustive]
    Inference(#[source] TranscribeError),
    #[error("{0}")]
    PendingPrecommitFailure(Arc<TakeFailure>),
    #[error(transparent)]
    Finalization(#[from] FinalizationError),
    #[error(transparent)]
    Mailbox(#[from] MailboxError),
}
#[derive(Debug)]
pub(crate) struct Session {
    pub(super) registration: Option<SessionRegistration>,
    pub(super) engine: Option<GenerationLease>,
    pub(super) ids: IdGenerator,
    pub(super) effective: EffectiveSession,
    pub(super) input: Option<UncommittedInput>,
    pub(super) current_epoch: Option<InterimEpoch>,
    pub(super) next_epoch: u64,
    pub(super) interim_task: Option<InterimTask>,
    /// End of the latest interim window submitted for decoding. Window ends
    /// only move forward, so a window ending no later holds no new speech.
    pub(super) last_interim_end: Option<u64>,
    pub(super) canceled_tasks: Vec<InterimTask>,
    pub(super) canceled_task_failed: bool,
    pub(super) committed: HashMap<String, CommittedItem>,
    pub(super) previous_item_id: Option<String>,
    pub(super) standard_interim_sent: String,
    pub(super) standard_interim_committed: String,
    pub(super) hypothesis_revision: u64,
    pub(super) last_hypothesis: Option<(InterimSnapshot, Option<HypothesisRanges>)>,
    /// Final outcomes applied to the take when its latest update was composed.
    pub(super) shown_finalized_seq: u64,
    /// End of the latest accepted interim window, where an update for landed
    /// final outcomes reports its empty span.
    pub(super) hypothesis_window_end: u64,
    pub(super) results: ResultMailbox,
}

impl Session {
    pub(super) fn empty(
        registration: SessionRegistration,
        engine: Option<GenerationLease>,
        ids: IdGenerator,
        effective: EffectiveSession,
    ) -> Self {
        Self {
            registration: Some(registration),
            engine,
            ids,
            effective,
            input: None,
            current_epoch: None,
            next_epoch: 1,
            interim_task: None,
            last_interim_end: None,
            canceled_tasks: Vec::with_capacity(SESSION_CANCEL_JOIN_CAPACITY),
            canceled_task_failed: false,
            committed: HashMap::with_capacity(MAX_COMMITTED_ITEMS_PER_SESSION),
            previous_item_id: None,
            standard_interim_sent: String::new(),
            standard_interim_committed: String::new(),
            hypothesis_revision: 0,
            last_hypothesis: None,
            shown_finalized_seq: 0,
            hypothesis_window_end: 0,
            results: ResultMailbox::default(),
        }
    }
}
