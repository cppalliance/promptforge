//! The conversation table: every running conversation by its id, which
//! is how a socket that reconnects finds its conversation again.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::conversation::Conversation;
use crate::protocol::ConversationId;

/// The running conversations by id, shared with each conversation so an
/// ended one removes itself.
pub(crate) type Table = Mutex<HashMap<ConversationId, Conversation>>;

/// Locks `table`. Every mutation is one insert or remove, so a poisoned
/// lock still holds a usable map.
pub(crate) fn lock(table: &Table) -> MutexGuard<'_, HashMap<ConversationId, Conversation>> {
    table.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Workshop's running conversations, keyed by the id each launch mints.
/// Cheap to clone; every clone is the same table.
#[derive(Clone, Default)]
pub struct Conversations {
    table: Arc<Table>,
}

impl fmt::Debug for Conversations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Conversations")
            .field("running", &lock(&self.table).len())
            .finish()
    }
}

impl Conversations {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a conversation running the agent `agent` under a fresh id,
    /// and holds it until its run ends or [`close`](Self::close) ends it.
    /// Its run has not started: hand [`Conversation::run`] the run's
    /// Harness.
    #[must_use]
    pub fn open(&self, agent: impl Into<String>) -> Conversation {
        let conversation = Conversation::new(
            ConversationId::fresh(),
            agent.into(),
            Arc::downgrade(&self.table),
        );
        lock(&self.table).insert(conversation.id().clone(), conversation.clone());
        conversation
    }

    /// The running conversation with this id, when one exists.
    #[must_use]
    pub fn get(&self, id: &ConversationId) -> Option<Conversation> {
        lock(&self.table).get(id).cloned()
    }

    /// Ends the conversation with this id: it leaves the table at once,
    /// and its run is cancelled. Returns whether a conversation was
    /// ended. A handle still held sees the state reach `Closed` once the
    /// run's outstanding effects are answered `Dropped`.
    #[must_use]
    pub fn close(&self, id: &ConversationId) -> bool {
        let Some(conversation) = lock(&self.table).remove(id) else {
            return false;
        };
        conversation.close();
        true
    }
}
