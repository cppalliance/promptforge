#[cfg(feature = "test-fixtures")]
use std::future::Future;

use super::state::{
    MAX_COMMITTED_ITEMS_PER_SESSION, SESSION_CANCEL_JOIN_CAPACITY, Session, SessionError,
};
use crate::realtime::item::{CommitReceipt, CommittedItem};
use crate::realtime::result_mailbox::{ItemFailure, ItemResult, MailboxError};

impl Session {
    pub(crate) fn commit(&mut self) -> Result<CommitReceipt, SessionError> {
        let input = self.input.as_ref().ok_or(SessionError::NoInput)?;
        input.validate_commit()?;
        let item_id = input.item_id().to_owned();
        if self.committed.len() == MAX_COMMITTED_ITEMS_PER_SESSION {
            return Err(SessionError::CommittedItemsAtCapacity);
        }
        if self.interim_task.is_some() && self.canceled_tasks.len() == SESSION_CANCEL_JOIN_CAPACITY
        {
            return Err(SessionError::CancelJoinAtCapacity);
        }

        self.invalidate_epoch()?;
        if let Some(task) = self.interim_task.take() {
            task.abort();
            self.canceled_tasks.push(task);
        }
        self.last_interim_window = None;
        let Some(input) = self.input.take() else {
            return Err(SessionError::NoInput);
        };
        let sealed = match input.seal() {
            Ok(sealed) => sealed,
            Err(failure) => {
                let (input, error) = *failure;
                self.input = Some(input);
                return Err(error.into());
            }
        };
        self.results.reserve_item(&item_id);
        let previous_item_id = self.previous_item_id.clone();
        let (mut item, pending_failure) = CommittedItem::from_sealed(sealed, previous_item_id);
        let receipt = item.receipt();
        self.previous_item_id = Some(item_id.clone());
        if let Some(failure) = pending_failure
            && let Some(terminal) = item.failed(ItemFailure::from_precommit(&failure))
        {
            self.results.set_terminal(&item_id, terminal)?;
        }
        let replaced = self.committed.insert(item_id, item);
        debug_assert!(replaced.is_none(), "opaque item IDs must be unique");
        Ok(receipt)
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn committed_count(&self) -> usize {
        self.committed.len()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn finalizing_count(&self) -> usize {
        self.committed
            .values()
            .filter(|item| item.is_finalizing())
            .count()
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn replace_finalization<F>(
        &mut self,
        item_id: &str,
        task: F,
    ) -> Result<(), SessionError>
    where
        F: Future<Output = Result<String, String>> + Send + 'static,
    {
        let item = self
            .committed
            .get_mut(item_id)
            .ok_or(MailboxError::UnknownItem)?;
        item.replace_finalization(tokio::spawn(task));
        Ok(())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn committed_prompt_and_guidance(&self, item_id: &str) -> Option<(&str, &[String])> {
        self.committed
            .get(item_id)
            .map(|item| (item.snapshot().prompt(), item.take().guidance()))
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn push_delta(
        &mut self,
        item_id: &str,
        transcript: String,
    ) -> Result<(), SessionError> {
        self.results.push_delta(item_id, transcript)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn replace_hypothesis(
        &mut self,
        item_id: &str,
        revision: u64,
        transcript: String,
    ) -> Result<(), SessionError> {
        self.results
            .replace_hypothesis(item_id, revision, transcript)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn finalize_completed(
        &mut self,
        item_id: &str,
        transcript: String,
    ) -> Result<(), SessionError> {
        let item = self
            .committed
            .get_mut(item_id)
            .ok_or(MailboxError::UnknownItem)?;
        let terminal = item
            .completed(transcript)
            .ok_or(MailboxError::TerminalAlreadySet)?;
        self.results.set_terminal(item_id, terminal)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn finalize_failed(
        &mut self,
        item_id: &str,
        message: String,
    ) -> Result<(), SessionError> {
        let item = self
            .committed
            .get_mut(item_id)
            .ok_or(MailboxError::UnknownItem)?;
        let terminal = item
            .failed(ItemFailure::TranscriptionFailed(message))
            .ok_or(MailboxError::TerminalAlreadySet)?;
        self.results.set_terminal(item_id, terminal)?;
        Ok(())
    }

    pub(crate) async fn finish_finalization(&mut self, item_id: &str) -> Result<(), SessionError> {
        let terminal = {
            let item = self
                .committed
                .get_mut(item_id)
                .ok_or(MailboxError::UnknownItem)?;
            item.finish_finalization()
                .await
                .map_err(SessionError::Finalization)?
        };
        self.results.set_terminal(item_id, terminal)?;
        Ok(())
    }

    pub(crate) fn drain_results(&mut self) -> Vec<ItemResult> {
        let results = self.results.drain();
        for result in results.iter().filter(|result| result.is_terminal()) {
            self.committed.remove(result.item_id());
        }
        results
    }
}

#[cfg(all(test, feature = "test-fixtures"))]
mod tests;
