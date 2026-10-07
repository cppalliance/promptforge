//! Per-item result mailbox buffering terminal transcription outcomes.

use std::collections::{HashMap, VecDeque};

use crate::take::TakeFailure;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ItemFailure {
    PrecommitTranscriptionFailed(String),
    TranscriptionFailed(String),
}

impl ItemFailure {
    pub(super) fn from_precommit(failure: &TakeFailure) -> Self {
        Self::PrecommitTranscriptionFailed(failure.to_string())
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn diagnostic(&self) -> &str {
        match self {
            Self::PrecommitTranscriptionFailed(message) | Self::TranscriptionFailed(message) => {
                message
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemResult {
    Completed {
        item_id: String,
        transcript: String,
        seconds: f64,
    },
    Failed {
        item_id: String,
        failure: ItemFailure,
    },
}

impl ItemResult {
    pub(super) fn item_id(&self) -> &str {
        match self {
            Self::Completed { item_id, .. } | Self::Failed { item_id, .. } => item_id,
        }
    }
}

#[derive(Debug, Default)]
struct ItemSlots {
    terminal: Option<ItemResult>,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum MailboxError {
    #[error("the committed item already reached a terminal outcome")]
    TerminalAlreadySet,
    #[error("the committed item is not active")]
    UnknownItem,
}

#[derive(Debug, Default)]
pub(super) struct ResultMailbox {
    slots: HashMap<String, ItemSlots>,
    terminal_order: VecDeque<String>,
}

impl ResultMailbox {
    pub(super) fn reserve_item(&mut self, item_id: &str) {
        let replaced = self.slots.insert(item_id.to_owned(), ItemSlots::default());
        debug_assert!(replaced.is_none(), "opaque item IDs must be unique");
    }

    #[cfg(test)]
    pub(super) fn reserved_items(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn set_terminal(
        &mut self,
        item_id: &str,
        result: ItemResult,
    ) -> Result<(), MailboxError> {
        debug_assert_eq!(result.item_id(), item_id);
        let slots = self
            .slots
            .get_mut(item_id)
            .ok_or(MailboxError::UnknownItem)?;
        if slots.terminal.is_some() {
            return Err(MailboxError::TerminalAlreadySet);
        }
        slots.terminal = Some(result);
        self.terminal_order.push_back(item_id.to_owned());
        Ok(())
    }

    pub(super) fn drain(&mut self) -> Vec<ItemResult> {
        let mut drained = Vec::new();
        while let Some(item_id) = self.terminal_order.pop_front() {
            if let Some(result) = self.slots.remove(&item_id).and_then(|slots| slots.terminal) {
                drained.push(result);
            }
        }
        drained
    }
}
