use super::{Session, SessionError};
use crate::realtime::result_mailbox::{ItemResult, SESSION_RESULT_CAPACITY};
use crate::realtime::session::state::InterimTaskOutput;
use crate::realtime::wire::ServerEvent;
use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy};
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
    pub(crate) fn schedule_interim(&mut self) -> Result<(), SessionError> {
        if self.interim_task.is_some() {
            return Ok(());
        }
        let Some(input) = self.input.as_ref() else {
            return Ok(());
        };
        let engine = self
            .engine
            .as_ref()
            .ok_or(SessionError::GenerationUnavailable)?;
        let window = input.take().interim_window(engine.window_samples())?;
        if window.samples.len() < EnginePolicy::MIN_WINDOW_SAMPLES
            || EnginePolicy::is_silence(window.samples.samples())
        {
            return Ok(());
        }
        let origin = (window.segment_start, window.start, window.end);
        if self.last_interim_window == Some(origin) {
            return Ok(());
        }
        let engine = engine.clone();
        let item_id = input.item_id().to_owned();
        let guidance = input.take().guidance().to_vec();
        let finalized = input.take().finalized();
        let epoch = self.begin_interim()?;
        self.last_interim_window = Some(origin);
        let (samples, samples_owner) = window.samples.into_decode();
        self.interim_task = Some(tokio::spawn(async move {
            let request = DecodeRequest::new(DecodeMode::Interim, samples, guidance, finalized)
                .with_lifetime_guard(samples_owner);
            let transcript = engine
                .decode(request)
                .await
                .map_err(|error| error.to_string());
            InterimTaskOutput::Decode {
                epoch,
                item_id,
                segment_start: window.segment_start,
                audio_start: window.start,
                audio_end: window.end,
                transcript,
            }
        }));
        Ok(())
    }
    pub(super) fn accept_scheduled_interim(
        &mut self,
        output: InterimTaskOutput,
    ) -> Result<Option<ServerEvent>, SessionError> {
        #[cfg(any(test, feature = "test-fixtures"))]
        let InterimTaskOutput::Decode {
            epoch,
            item_id,
            segment_start,
            audio_start,
            audio_end,
            transcript,
        } = output
        else {
            unreachable!("fixture interims are accepted by the fixture path");
        };
        #[cfg(not(any(test, feature = "test-fixtures")))]
        let InterimTaskOutput::Decode {
            epoch,
            item_id,
            segment_start,
            audio_start,
            audio_end,
            transcript,
        } = output;
        if self.current_epoch != Some(epoch) {
            return Ok(None);
        }
        let input = self.input.as_ref().ok_or(SessionError::NoInput)?;
        if input.item_id() != item_id {
            return Ok(None);
        }
        let transcript = transcript.map_err(|_| SessionError::Inference)?;
        if transcript.is_empty() {
            return Ok(None);
        }
        let include_hypothesis = input.snapshot().include_hypothesis();
        let update =
            input
                .take()
                .next_window_snapshot(&transcript, segment_start, audio_start, audio_end);
        if !include_hypothesis {
            if let Some(snapshot) = update {
                let committed = snapshot.committed();
                if let Some(delta) = committed.strip_prefix(&self.standard_interim_committed) {
                    if !delta.is_empty() {
                        self.pending_interim.push(delta.to_owned());
                    }
                    committed.clone_into(&mut self.standard_interim_committed);
                }
            }
            return Ok(None);
        }
        self.hypothesis_revision = self
            .hypothesis_revision
            .checked_add(1)
            .ok_or(SessionError::EpochExhausted)?;
        let Some(snapshot) = update else {
            return Ok(None);
        };
        Ok(Some(ServerEvent::hypothesis(
            self.ids.event(),
            input.item_id().to_owned(),
            self.hypothesis_revision,
            snapshot,
            sample_millis(audio_start),
            sample_millis(audio_end),
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
}

fn sample_millis(samples: u64) -> u64 {
    let millis = u128::from(samples) * 1_000 / EnginePolicy::SAMPLE_RATE as u128;
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
