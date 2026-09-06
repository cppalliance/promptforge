use std::sync::Arc;

use tokio::task::JoinHandle;

use super::input::{InputSnapshot, SealedInput};
use super::result_mailbox::ItemResult;
use crate::take::Take;

type FinalizationTask = JoinHandle<Result<String, String>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitReceipt {
    item_id: String,
    previous_item_id: Option<String>,
}

impl CommitReceipt {
    pub(crate) fn new(item_id: String, previous_item_id: Option<String>) -> Self {
        Self {
            item_id,
            previous_item_id,
        }
    }

    pub(crate) fn item_id(&self) -> &str {
        &self.item_id
    }

    pub(crate) fn previous_item_id(&self) -> Option<&str> {
        self.previous_item_id.as_deref()
    }
}

#[derive(Debug)]
pub(crate) struct CommittedItem {
    id: String,
    previous_item_id: Option<String>,
    snapshot: InputSnapshot,
    take: Arc<Take>,
    duration_seconds: f64,
    finalization: Option<FinalizationTask>,
    terminal: bool,
}

impl CommittedItem {
    pub(crate) fn from_sealed(
        sealed: SealedInput,
        previous_item_id: Option<String>,
    ) -> (Self, Option<String>) {
        let pending_failure = sealed.take.pending_failure();
        let take = Arc::new(sealed.take);
        let finalization = if pending_failure.is_none() {
            take.finalization().map(tokio::spawn)
        } else {
            None
        };
        (
            Self {
                id: sealed.item_id,
                previous_item_id,
                snapshot: sealed.snapshot,
                take,
                duration_seconds: sealed.duration_seconds,
                finalization,
                terminal: false,
            },
            pending_failure,
        )
    }

    pub(crate) fn receipt(&self) -> CommitReceipt {
        CommitReceipt::new(self.id.clone(), self.previous_item_id.clone())
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) const fn snapshot(&self) -> &InputSnapshot {
        &self.snapshot
    }

    pub(crate) fn take(&self) -> &Take {
        &self.take
    }

    pub(crate) const fn is_finalizing(&self) -> bool {
        self.finalization.is_some()
    }

    pub(crate) const fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub(crate) async fn finish_finalization(&mut self) -> Result<ItemResult, String> {
        let Some(task) = self.finalization.as_mut() else {
            return Err("the committed item has no active finalization".to_owned());
        };
        let outcome = task
            .await
            .map_err(|error| format!("committed item finalization task failed: {error}"))?;
        self.finalization = None;
        match outcome {
            Ok(transcript) => self
                .completed(transcript)
                .ok_or_else(|| "the committed item already reached a terminal outcome".to_owned()),
            Err(message) => self
                .failed(message)
                .ok_or_else(|| "the committed item already reached a terminal outcome".to_owned()),
        }
    }

    pub(crate) fn take_finalization(&mut self) -> Option<FinalizationTask> {
        self.finalization.take()
    }

    pub(crate) fn completed(&mut self, transcript: String) -> Option<ItemResult> {
        if std::mem::replace(&mut self.terminal, true) {
            return None;
        }
        Some(ItemResult::Completed {
            item_id: self.id.clone(),
            transcript,
            seconds: self.duration_seconds,
        })
    }

    pub(crate) fn failed(&mut self, message: String) -> Option<ItemResult> {
        if std::mem::replace(&mut self.terminal, true) {
            return None;
        }
        Some(ItemResult::Failed {
            item_id: self.id.clone(),
            message,
        })
    }
}

impl Drop for CommittedItem {
    fn drop(&mut self) {
        if let Some(task) = &self.finalization {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CommitReceipt;

    #[test]
    fn miri_commit_receipt_preserves_provisional_id_and_lineage() {
        let receipt = CommitReceipt::new("item_two".to_owned(), Some("item_one".to_owned()));
        assert_eq!(receipt.item_id(), "item_two");
        assert_eq!(receipt.previous_item_id(), Some("item_one"));
    }
}
