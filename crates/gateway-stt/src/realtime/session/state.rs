use crate::audio::AudioError;
use crate::generation::GenerationLease;
use crate::realtime::input::UncommittedInput;
use crate::realtime::item::CommittedItem;
use crate::realtime::registry::SessionRegistration;
use crate::realtime::result_mailbox::{MailboxError, ResultMailbox};
use crate::realtime::wire::{EffectiveSession, IdGenerator};
use gateway_stt_engine::TranscribeError;
use std::collections::HashMap;
use tokio::task::JoinHandle;
pub(super) const SESSION_CANCEL_JOIN_CAPACITY: usize = 8;
pub(super) const MAX_COMMITTED_ITEMS_PER_SESSION: usize = 4;
pub(super) type InterimTask = JoinHandle<InterimTaskOutput>;
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
        transcript: Result<String, TranscribeError>,
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
    #[error("the realtime session result capacity is reached")]
    InterimAtCapacity,
    #[error("{0}")]
    PendingPrecommitFailure(String),
    #[error("{0}")]
    Finalization(String),
    #[error(transparent)]
    Mailbox(MailboxError),
}

impl From<MailboxError> for SessionError {
    fn from(error: MailboxError) -> Self {
        #[cfg(any(test, feature = "test-fixtures"))]
        if error == MailboxError::ResultAtCapacity {
            return Self::InterimAtCapacity;
        }
        Self::Mailbox(error)
    }
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
    pub(super) last_interim_window: Option<(u64, u64, u64)>,
    pub(super) canceled_tasks: Vec<InterimTask>,
    pub(super) canceled_task_failed: bool,
    pub(super) committed: HashMap<String, CommittedItem>,
    pub(super) previous_item_id: Option<String>,
    pub(super) pending_interim: Vec<String>,
    pub(super) standard_interim_committed: String,
    pub(super) hypothesis_revision: u64,
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
            last_interim_window: None,
            canceled_tasks: Vec::with_capacity(SESSION_CANCEL_JOIN_CAPACITY),
            canceled_task_failed: false,
            committed: HashMap::with_capacity(MAX_COMMITTED_ITEMS_PER_SESSION),
            previous_item_id: None,
            pending_interim: Vec::new(),
            standard_interim_committed: String::new(),
            hypothesis_revision: 0,
            results: ResultMailbox::default(),
        }
    }
}
