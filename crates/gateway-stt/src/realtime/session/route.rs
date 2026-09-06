use std::time::Duration;

use gateway_stt_engine::{DecodeMode, DecodeRequest};

use super::{Session, SessionError};
use crate::realtime::result_mailbox::{ItemResult, SESSION_RESULT_CAPACITY};
use crate::realtime::wire::ServerEvent;

impl Session {
    pub(crate) fn created_event(&self) -> ServerEvent {
        ServerEvent::session_created(self.ids.event(), self.effective.clone())
    }

    pub(crate) fn updated_event(&self) -> ServerEvent {
        ServerEvent::session_updated(self.ids.event(), self.effective.clone())
    }

    pub(crate) fn cleared_event(&self) -> ServerEvent {
        ServerEvent::input_cleared(self.ids.event())
    }

    pub(crate) fn next_event_id(&self) -> String {
        self.ids.event()
    }

    pub(crate) fn ensure_interim_capacity(&self) -> Result<(), SessionError> {
        let standard_client = self.input.as_ref().map_or_else(
            || !self.effective.includes_hypothesis(),
            |input| !input.snapshot().include_hypothesis(),
        );
        if standard_client && self.pending_interim.len() == SESSION_RESULT_CAPACITY {
            return Err(SessionError::InterimAtCapacity);
        }
        Ok(())
    }

    pub(crate) async fn decode_interim(&mut self) -> Result<Option<ServerEvent>, SessionError> {
        let input = self.input.as_ref().ok_or(SessionError::NoInput)?;
        let include_hypothesis = input.snapshot().include_hypothesis();
        let engine = self
            .engine
            .as_ref()
            .ok_or(SessionError::GenerationUnavailable)?;
        let transcript = engine
            .decode(DecodeRequest::new(
                DecodeMode::Interim,
                input.take().uncommitted_snapshot(engine.window_samples()),
                input.take().guidance().to_vec(),
                input.take().finalized(),
            ))
            .await
            .map_err(|_| SessionError::Inference)?;
        if transcript.is_empty() {
            return Ok(None);
        }
        let finalized = input.take().finalized();
        let update = input.take().next_interim(&transcript);
        if !include_hypothesis {
            if let Some((committed, _)) = update {
                let delta = committed
                    .strip_prefix(&self.standard_interim_committed)
                    .ok_or(SessionError::Inference)?;
                if !delta.is_empty() {
                    self.pending_interim.push(delta.to_owned());
                }
                self.standard_interim_committed = committed;
            }
            return Ok(None);
        }
        self.hypothesis_revision = self
            .hypothesis_revision
            .checked_add(1)
            .ok_or(SessionError::EpochExhausted)?;
        let (agreed, tentative) = update.unwrap_or_else(|| (String::new(), transcript));
        Ok(Some(ServerEvent::hypothesis(
            self.ids.event(),
            input.item_id().to_owned(),
            self.hypothesis_revision,
            finalized,
            agreed,
            tentative,
            u64::try_from(Duration::from_secs_f64(input.buffered_duration_seconds()).as_millis())
                .unwrap_or(u64::MAX),
        )))
    }

    pub(crate) fn take_pending_interim(&mut self, item_id: &str) -> Vec<ServerEvent> {
        self.hypothesis_revision = 0;
        self.standard_interim_committed.clear();
        self.pending_interim
            .drain(..)
            .map(|transcript| {
                ServerEvent::transcription_delta(self.ids.event(), item_id.to_owned(), transcript)
            })
            .collect()
    }

    pub(crate) fn committed_events(
        &self,
        receipt: &crate::realtime::CommitReceipt,
    ) -> [ServerEvent; 2] {
        ServerEvent::committed(
            self.ids.event(),
            self.ids.event(),
            receipt.item_id().to_owned(),
            receipt.previous_item_id().map(str::to_owned),
        )
    }

    pub(crate) fn drain_events(&mut self) -> Vec<ServerEvent> {
        self.drain_results()
            .into_iter()
            .map(|result: ItemResult| ServerEvent::item_result(self.ids.event(), result))
            .collect()
    }

    pub(crate) async fn finish_ready(&mut self) -> Result<Vec<ServerEvent>, SessionError> {
        let ready = self
            .committed
            .values()
            .filter(|item| item.finalization_finished())
            .map(|item| item.id().to_owned())
            .collect::<Vec<_>>();
        for item_id in ready {
            self.finish_finalization(&item_id).await?;
        }
        Ok(self.drain_events())
    }

    pub(crate) fn replacement_events(&self) -> Vec<ServerEvent> {
        let mut events = self
            .committed
            .values()
            .filter(|item| !item.is_terminal())
            .map(|item| ServerEvent::engine_replaced_item(self.ids.event(), item.id().to_owned()))
            .collect::<Vec<_>>();
        if self.input.is_some() {
            events.push(ServerEvent::engine_replaced(self.ids.event()));
        }
        events
    }
}
