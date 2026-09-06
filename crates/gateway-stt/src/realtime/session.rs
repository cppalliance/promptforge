use std::future::Future;

use super::input::{InputSnapshot, UncommittedInput};
use super::item::CommittedItem;
use super::registry::SessionRegistration;
use super::wire::{ClientError, EffectiveSession, IdGenerator, ServerEvent};
use crate::generation::GenerationLease;

mod items;
mod route;
mod state;

#[cfg(test)]
use state::MAX_COMMITTED_ITEMS_PER_SESSION;
use state::SESSION_CANCEL_JOIN_CAPACITY;
pub(crate) use state::{InterimEpoch, Session, SessionError};

impl Session {
    pub(crate) fn new(registration: SessionRegistration, engine: Option<GenerationLease>) -> Self {
        let ids = IdGenerator::default();
        let effective = EffectiveSession::new(ids.session());
        Self::empty(registration, engine, ids, effective)
    }

    pub(crate) fn update_text(&mut self, text: &str) -> Result<(), ClientError> {
        self.effective.apply_update_text(text)
    }

    pub(crate) fn append_base64(&mut self, payload: &str) -> Result<(), SessionError> {
        if let Some(input) = &mut self.input {
            if let Some(failure) = input.pending_failure() {
                return Err(SessionError::PendingPrecommitFailure(failure));
            }
            return input.append_base64(payload).map_err(SessionError::from);
        }

        let snapshot = InputSnapshot::new(
            self.effective.prompt().to_owned(),
            self.effective.includes_hypothesis(),
        );
        let input = UncommittedInput::first_append(
            self.ids.item(),
            snapshot,
            self.engine.clone(),
            payload,
        )?;
        self.input = Some(input);
        Ok(())
    }

    pub(crate) const fn input(&self) -> Option<&UncommittedInput> {
        self.input.as_ref()
    }

    pub(crate) fn clear(&mut self) -> Result<(), SessionError> {
        if self.input.is_none() {
            return Ok(());
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
        self.input = None;
        self.pending_interim.clear();
        self.standard_interim_committed.clear();
        self.hypothesis_revision = 0;
        Ok(())
    }

    pub(crate) fn begin_interim(&mut self) -> Result<InterimEpoch, SessionError> {
        if self.input.is_none() {
            return Err(SessionError::NoInput);
        }
        let epoch = InterimEpoch(self.next_epoch);
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .ok_or(SessionError::EpochExhausted)?;
        self.current_epoch = Some(epoch);
        Ok(epoch)
    }

    pub(crate) fn spawn_interim<F>(&mut self, task: F) -> Result<InterimEpoch, SessionError>
    where
        F: Future<Output = String> + Send + 'static,
    {
        if self.interim_task.is_some() && self.canceled_tasks.len() == SESSION_CANCEL_JOIN_CAPACITY
        {
            return Err(SessionError::CancelJoinAtCapacity);
        }
        if let Some(previous) = self.interim_task.take() {
            previous.abort();
            self.canceled_tasks.push(previous);
        }
        let epoch = self.begin_interim()?;
        self.interim_task = Some(tokio::spawn(async move { (epoch, task.await) }));
        Ok(epoch)
    }

    pub(crate) fn accept_interim(
        &mut self,
        epoch: InterimEpoch,
        transcript: String,
    ) -> Option<ServerEvent> {
        if self.current_epoch != Some(epoch) {
            return None;
        }
        let item_id = self.input.as_ref()?.item_id().to_owned();
        Some(ServerEvent::transcription_delta(
            self.ids.event(),
            item_id,
            transcript,
        ))
    }

    pub(crate) async fn finish_interim(&mut self) -> Result<Option<ServerEvent>, SessionError> {
        let Some(task) = self.interim_task.as_mut() else {
            return Ok(None);
        };
        let result = task.await;
        self.interim_task = None;
        let (epoch, transcript) = result.map_err(|_| SessionError::CanceledTaskFailed)?;
        Ok(self.accept_interim(epoch, transcript))
    }

    pub(crate) const fn canceled_join_count(&self) -> usize {
        self.canceled_tasks.len()
    }

    pub(crate) fn record_pending_failure(&mut self, failure: String) -> Result<(), SessionError> {
        let input = self.input.as_mut().ok_or(SessionError::NoInput)?;
        input.record_pending_failure(failure);
        Ok(())
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn pending_failure(&self) -> Option<String> {
        self.input
            .as_ref()
            .and_then(UncommittedInput::pending_failure)
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn pending_final_segments(&self) -> Option<usize> {
        self.input
            .as_ref()
            .map(|input| input.take().pending_final_segments())
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn allocated_event_count(&self) -> u64 {
        self.ids.event_count()
    }

    pub(crate) async fn join_canceled(&mut self) -> Result<(), SessionError> {
        while let Some(task) = self.canceled_tasks.first_mut() {
            let result = task.await;
            self.canceled_tasks.remove(0);
            if result.is_err_and(|error| !error.is_cancelled()) {
                self.canceled_task_failed = true;
            }
        }
        if self.canceled_task_failed {
            self.canceled_task_failed = false;
            Err(SessionError::CanceledTaskFailed)
        } else {
            Ok(())
        }
    }

    fn invalidate_epoch(&mut self) -> Result<(), SessionError> {
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .ok_or(SessionError::EpochExhausted)?;
        self.current_epoch = None;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut interim_tasks = Vec::with_capacity(self.canceled_tasks.len() + 1);
        if let Some(task) = self.interim_task.take() {
            interim_tasks.push(task);
        }
        interim_tasks.append(&mut self.canceled_tasks);
        let finalization_tasks = self
            .committed
            .values_mut()
            .filter_map(CommittedItem::take_finalization)
            .collect();
        if let Some(mut registration) = self.registration.take() {
            registration.retire(interim_tasks, finalization_tasks);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use base64::Engine as _;

    use super::{
        MAX_COMMITTED_ITEMS_PER_SESSION, SESSION_CANCEL_JOIN_CAPACITY, Session, SessionError,
    };
    use crate::realtime::registry::SessionRegistry;

    fn encoded(samples: &[i16]) -> String {
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn update(prompt: &str, include: bool) -> String {
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {"input": {"transcription": {"prompt": prompt}}},
                "include": if include {
                    vec!["item.input_audio_transcription.hypothesis"]
                } else {
                    Vec::<&str>::new()
                }
            }
        })
        .to_string()
    }

    fn session() -> Session {
        let registration = SessionRegistry::default()
            .register()
            .expect("session registers");
        Session::new(registration, None)
    }

    #[test]
    fn first_successful_append_freezes_configuration_until_clear() {
        let mut session = session();
        session
            .update_text(&update("first", true))
            .expect("first update applies");
        session
            .append_base64(&encoded(&[1, 2, 3]))
            .expect("first append succeeds");
        let first_item = session.input().expect("input exists").item_id().to_owned();

        session
            .update_text(&update("second", false))
            .expect("second update applies");
        let input = session.input().expect("input remains");
        assert_eq!(input.item_id(), first_item);
        assert_eq!(input.snapshot().prompt(), "first");
        assert!(input.snapshot().include_hypothesis());

        session.clear().expect("clear succeeds");
        session
            .append_base64(&encoded(&[4, 5, 6]))
            .expect("next input appends");
        let input = session.input().expect("replacement input exists");
        assert_ne!(input.item_id(), first_item);
        assert_eq!(input.snapshot().prompt(), "second");
        assert!(!input.snapshot().include_hypothesis());
    }

    #[test]
    fn failed_first_append_does_not_capture_a_snapshot() {
        let mut session = session();
        session
            .update_text(&update("before", false))
            .expect("update applies");
        assert!(session.append_base64("not base64").is_err());
        assert!(session.input().is_none());

        session
            .update_text(&update("after", true))
            .expect("replacement update applies");
        session
            .append_base64(&encoded(&[0, 1]))
            .expect("valid append succeeds");
        assert_eq!(
            session.input().expect("input exists").snapshot().prompt(),
            "after"
        );
    }

    #[test]
    fn clear_retires_only_input_and_rejects_stale_interim_epochs() {
        let mut session = session();
        session
            .append_base64(&encoded(&vec![0; 2_400]))
            .expect("audio appends");
        let epoch = session.begin_interim().expect("epoch begins");
        assert!(
            session
                .accept_interim(epoch, "current".to_owned())
                .is_some()
        );

        session.clear().expect("clear succeeds");
        assert!(session.input().is_none());
        assert!(session.accept_interim(epoch, "stale".to_owned()).is_none());

        session
            .append_base64(&encoded(&vec![0; 2_400]))
            .expect("replacement audio appends");
        let next = session.begin_interim().expect("new epoch begins");
        assert_ne!(next, epoch);
        assert!(session.accept_interim(epoch, "stale".to_owned()).is_none());
        assert!(session.accept_interim(next, "fresh".to_owned()).is_some());
    }

    #[test]
    fn miri_interim_epoch_rejects_results_after_clear_and_reuse() {
        let mut session = session();
        session
            .append_base64(&encoded(&[0, 0]))
            .expect("input appends");
        let stale = session.begin_interim().expect("first epoch begins");
        session.clear().expect("input clears");
        session
            .append_base64(&encoded(&[0, 0]))
            .expect("replacement input appends");
        let current = session.begin_interim().expect("next epoch begins");

        assert!(session.accept_interim(stale, "stale".to_owned()).is_none());
        assert!(
            session
                .accept_interim(current, "current".to_owned())
                .is_some()
        );
    }

    #[test]
    fn miri_commit_reserves_capacity_promotes_ids_and_keeps_lineage() {
        let mut session = session();
        let mut previous = None;
        let mut committed = Vec::new();
        for _ in 0..MAX_COMMITTED_ITEMS_PER_SESSION {
            session
                .append_base64(&encoded(&vec![0; 2_400]))
                .expect("committable input appends");
            let provisional = session.input().expect("input exists").item_id().to_owned();
            let receipt = session.commit().expect("item commits within capacity");
            assert_eq!(receipt.item_id(), provisional);
            assert_eq!(receipt.previous_item_id(), previous.as_deref());
            previous = Some(provisional.clone());
            committed.push(provisional);
        }

        session
            .append_base64(&encoded(&vec![0; 2_400]))
            .expect("retry input appends");
        let retry_id = session
            .input()
            .expect("retry input exists")
            .item_id()
            .to_owned();
        assert_eq!(
            session.commit(),
            Err(SessionError::CommittedItemsAtCapacity)
        );
        assert_eq!(session.input().expect("input remains").item_id(), retry_id);

        session
            .finalize_completed(&committed[0], "done".to_owned())
            .expect("item finalizes");
        session.drain_results();
        assert_eq!(session.commit().expect("retry commits").item_id(), retry_id);
    }

    #[tokio::test]
    async fn canceled_task_joins_accept_exact_capacity_and_reject_next() {
        assert_eq!(SESSION_CANCEL_JOIN_CAPACITY, 8);
        let mut session = session();
        for _ in 0..SESSION_CANCEL_JOIN_CAPACITY {
            session
                .append_base64(&encoded(&[0, 0]))
                .expect("input appends");
            session
                .spawn_interim(pending())
                .expect("task starts within join capacity");
            session.clear().expect("task is retained for joining");
        }
        assert_eq!(session.canceled_join_count(), SESSION_CANCEL_JOIN_CAPACITY);

        session
            .append_base64(&encoded(&[0, 0]))
            .expect("capacity-plus-one input appends");
        session
            .spawn_interim(pending())
            .expect("capacity-plus-one task starts");
        assert_eq!(session.clear(), Err(SessionError::CancelJoinAtCapacity));
        assert!(
            session.input().is_some(),
            "recoverable error preserves input"
        );

        session.join_canceled().await.expect("canceled tasks join");
        session.clear().expect("retry succeeds after joins drain");
    }

    #[test]
    fn clear_resets_partial_pcm_and_resampler_state() {
        let mut reused = session();
        reused
            .append_base64(&base64::engine::general_purpose::STANDARD.encode([0x7f]))
            .expect("odd byte appends");
        reused.clear().expect("partial input clears");
        reused
            .append_base64(&encoded(&vec![123; 2_400]))
            .expect("clean input appends");

        let mut fresh = session();
        fresh
            .append_base64(&encoded(&vec![123; 2_400]))
            .expect("fresh input appends");
        assert_eq!(
            reused
                .input()
                .expect("reused input")
                .take()
                .uncommitted_snapshot(usize::MAX),
            fresh
                .input()
                .expect("fresh input")
                .take()
                .uncommitted_snapshot(usize::MAX)
        );
    }
}
