//! Realtime session lifecycle for input appends, clears, and interim epochs.

use super::input::{InputSnapshot, UncommittedInput};
use super::item::CommittedItem;
use super::registry::SessionRegistration;
use super::wire::{ClientError, EffectiveSession, IdGenerator, ServerEvent};
use crate::generation::GenerationLease;
use crate::take::TakeFailure;
#[cfg(any(test, feature = "test-fixtures"))]
use std::future::Future;
mod items;
mod route;
mod state;

use state::InterimTaskOutput;
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

        let input = UncommittedInput::first_append(
            self.ids.item(),
            self.input_snapshot(),
            self.engine.clone(),
            payload,
        )?;
        self.input = Some(input);
        Ok(())
    }

    /// Appends like [`append_base64`](Self::append_base64), except that a
    /// take this append starts classifies speech with `detector`.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn append_base64_detecting(
        &mut self,
        payload: &str,
        detector: gateway_stt_engine::FallbackDetector,
    ) -> Result<(), SessionError> {
        if self.input.is_some() {
            return self.append_base64(payload);
        }
        let input = UncommittedInput::first_append_with_detector(
            self.ids.item(),
            self.input_snapshot(),
            self.engine.clone(),
            payload,
            detector,
        )?;
        self.input = Some(input);
        Ok(())
    }

    fn input_snapshot(&self) -> InputSnapshot {
        InputSnapshot::new(
            self.effective.prompt().to_owned(),
            self.effective.hypothesis_include(),
        )
    }

    #[cfg(any(test, feature = "test-fixtures"))]
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
        self.last_interim_end = None;
        self.standard_interim_sent.clear();
        self.standard_interim_committed.clear();
        self.hypothesis_revision = 0;
        self.last_hypothesis = None;
        self.shown_finalized_seq = 0;
        self.hypothesis_window_end = 0;
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

    #[cfg(any(test, feature = "test-fixtures"))]
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
        self.interim_task = Some(tokio::spawn(async move {
            InterimTaskOutput::Fixture(epoch, task.await)
        }));
        Ok(epoch)
    }

    #[cfg(any(test, feature = "test-fixtures"))]
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
        match result.map_err(|_| SessionError::CanceledTaskFailed)? {
            #[cfg(any(test, feature = "test-fixtures"))]
            InterimTaskOutput::Fixture(epoch, transcript) => {
                Ok(self.accept_interim(epoch, transcript))
            }
            output @ InterimTaskOutput::Decode { .. } => self.accept_scheduled_interim(output),
        }
    }

    pub(super) fn interim_finished(&self) -> bool {
        self.interim_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) const fn canceled_join_count(&self) -> usize {
        self.canceled_tasks.len()
    }

    pub(crate) fn record_pending_failure(
        &mut self,
        failure: TakeFailure,
    ) -> Result<(), SessionError> {
        let input = self.input.as_mut().ok_or(SessionError::NoInput)?;
        input.record_pending_failure(failure);
        Ok(())
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn pending_failure(&self) -> Option<String> {
        self.input
            .as_ref()
            .and_then(UncommittedInput::pending_failure)
            .map(|failure| failure.to_string())
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

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) async fn join_canceled(&mut self) -> Result<(), SessionError> {
        while let Some(task) = self.canceled_tasks.first_mut() {
            let result = task.await;
            self.canceled_tasks.remove(0);
            self.record_canceled_result(result);
        }
        self.take_canceled_failure()
    }

    pub(super) async fn reap_canceled(&mut self) -> Result<(), SessionError> {
        while self
            .canceled_tasks
            .first()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            let Some(task) = self.canceled_tasks.first_mut() else {
                break;
            };
            let result = task.await;
            self.canceled_tasks.remove(0);
            self.record_canceled_result(result);
        }
        self.take_canceled_failure()
    }

    fn record_canceled_result(
        &mut self,
        result: Result<InterimTaskOutput, tokio::task::JoinError>,
    ) {
        if result.is_err_and(|error| !error.is_cancelled()) {
            self.canceled_task_failed = true;
        }
    }

    fn take_canceled_failure(&mut self) -> Result<(), SessionError> {
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
mod tests;
