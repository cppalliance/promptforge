use std::collections::{HashMap, VecDeque};

pub(crate) const SESSION_RESULT_CAPACITY: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ItemFailure {
    FinalSegmentOverload(String),
    PrecommitTranscriptionFailed(String),
    TranscriptionFailed(String),
}

impl ItemFailure {
    pub(crate) fn from_precommit(message: &str) -> Self {
        if message == "final segment capacity is reached" {
            Self::FinalSegmentOverload(message.to_owned())
        } else {
            Self::PrecommitTranscriptionFailed(message.to_owned())
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn diagnostic(&self) -> &str {
        match self {
            Self::FinalSegmentOverload(message)
            | Self::PrecommitTranscriptionFailed(message)
            | Self::TranscriptionFailed(message) => message,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemResult {
    #[cfg(any(test, feature = "test-fixtures"))]
    Delta { item_id: String, transcript: String },
    #[cfg(any(test, feature = "test-fixtures"))]
    Hypothesis {
        item_id: String,
        revision: u64,
        transcript: String,
    },
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
    pub(crate) fn item_id(&self) -> &str {
        match self {
            #[cfg(any(test, feature = "test-fixtures"))]
            Self::Delta { item_id, .. } | Self::Hypothesis { item_id, .. } => item_id,
            Self::Completed { item_id, .. } | Self::Failed { item_id, .. } => item_id,
        }
    }

    pub(crate) const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

#[derive(Debug, Default)]
struct ItemSlots {
    #[cfg(any(test, feature = "test-fixtures"))]
    hypothesis: Option<ItemResult>,
    terminal: Option<ItemResult>,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum MailboxError {
    #[cfg(any(test, feature = "test-fixtures"))]
    #[error("the realtime session result capacity is reached")]
    ResultAtCapacity,
    #[error("the committed item already reached a terminal outcome")]
    TerminalAlreadySet,
    #[error("the committed item is not active")]
    UnknownItem,
}

#[derive(Debug, Default)]
pub(crate) struct ResultMailbox {
    #[cfg(any(test, feature = "test-fixtures"))]
    results: VecDeque<ItemResult>,
    slots: HashMap<String, ItemSlots>,
    #[cfg(any(test, feature = "test-fixtures"))]
    hypothesis_order: VecDeque<String>,
    terminal_order: VecDeque<String>,
}

impl ResultMailbox {
    pub(crate) fn reserve_item(&mut self, item_id: &str) {
        let replaced = self.slots.insert(item_id.to_owned(), ItemSlots::default());
        debug_assert!(replaced.is_none(), "opaque item IDs must be unique");
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn push_delta(
        &mut self,
        item_id: &str,
        transcript: String,
    ) -> Result<(), MailboxError> {
        let slots = self.slots.get(item_id).ok_or(MailboxError::UnknownItem)?;
        if slots.terminal.is_some() {
            return Err(MailboxError::TerminalAlreadySet);
        }
        if self.results.len() == SESSION_RESULT_CAPACITY {
            return Err(MailboxError::ResultAtCapacity);
        }
        self.results.push_back(ItemResult::Delta {
            item_id: item_id.to_owned(),
            transcript,
        });
        Ok(())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn replace_hypothesis(
        &mut self,
        item_id: &str,
        revision: u64,
        transcript: String,
    ) -> Result<(), MailboxError> {
        let slots = self
            .slots
            .get_mut(item_id)
            .ok_or(MailboxError::UnknownItem)?;
        if slots.terminal.is_some() {
            return Err(MailboxError::TerminalAlreadySet);
        }
        if slots.hypothesis.is_none() {
            self.hypothesis_order.push_back(item_id.to_owned());
        }
        slots.hypothesis = Some(ItemResult::Hypothesis {
            item_id: item_id.to_owned(),
            revision,
            transcript,
        });
        Ok(())
    }

    pub(crate) fn set_terminal(
        &mut self,
        item_id: &str,
        result: ItemResult,
    ) -> Result<(), MailboxError> {
        debug_assert!(result.is_terminal());
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

    pub(crate) fn drain(&mut self) -> Vec<ItemResult> {
        #[cfg(any(test, feature = "test-fixtures"))]
        let mut drained = self.results.drain(..).collect::<Vec<_>>();
        #[cfg(not(any(test, feature = "test-fixtures")))]
        let mut drained = Vec::new();
        #[cfg(any(test, feature = "test-fixtures"))]
        while let Some(item_id) = self.hypothesis_order.pop_front() {
            if let Some(result) = self
                .slots
                .get_mut(&item_id)
                .and_then(|slots| slots.hypothesis.take())
            {
                drained.push(result);
            }
        }
        while let Some(item_id) = self.terminal_order.pop_front() {
            if let Some(result) = self
                .slots
                .get_mut(&item_id)
                .and_then(|slots| slots.terminal.take())
            {
                drained.push(result);
            }
        }
        for result in drained.iter().filter(|result| result.is_terminal()) {
            self.slots.remove(result.item_id());
        }
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemResult, MailboxError, ResultMailbox, SESSION_RESULT_CAPACITY};

    #[test]
    fn miri_result_mailbox_bounds_results_and_reserves_terminal_and_hypothesis_slots() {
        let mut mailbox = ResultMailbox::default();
        mailbox.reserve_item("item");
        for index in 0..SESSION_RESULT_CAPACITY {
            mailbox
                .push_delta("item", index.to_string())
                .expect("ordinary result fits");
        }
        assert_eq!(
            mailbox.push_delta("item", "overflow".to_owned()),
            Err(MailboxError::ResultAtCapacity)
        );
        mailbox
            .replace_hypothesis("item", 1, "old".to_owned())
            .expect("hypothesis uses its slot");
        mailbox
            .replace_hypothesis("item", 2, "new".to_owned())
            .expect("hypothesis is replaceable");
        mailbox
            .set_terminal(
                "item",
                ItemResult::Completed {
                    item_id: "item".to_owned(),
                    transcript: "done".to_owned(),
                    seconds: 0.1,
                },
            )
            .expect("terminal uses its reserved slot");

        let results = mailbox.drain();
        assert_eq!(results.len(), SESSION_RESULT_CAPACITY + 2);
        assert!(matches!(
            &results[SESSION_RESULT_CAPACITY],
            ItemResult::Hypothesis {
                revision: 2,
                transcript,
                ..
            } if transcript == "new"
        ));
        assert!(results.last().is_some_and(ItemResult::is_terminal));
    }
}
