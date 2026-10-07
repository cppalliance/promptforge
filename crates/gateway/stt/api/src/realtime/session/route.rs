//! Session-side server event emission and interim decode scheduling.

use super::{Session, SessionError};
use crate::realtime::result_mailbox::ItemResult;
use crate::realtime::session::state::InterimTaskOutput;
use crate::realtime::wire::{HypothesisRanges, ServerEvent};
use crate::take::{FinalizedRange, InterimSnapshot, token_spans};
use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, TranscribeError};

/// Trailing agreed words a plain client receives only once later agreement
/// extends past them or the input commits, because an aligned final rewrite
/// can still revise them.
const STANDARD_HOLD_BACK_WORDS: usize = 2;

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
            || self.last_interim_end.is_some_and(|end| window.end <= end)
        {
            return Ok(());
        }
        let engine = engine.clone();
        let item_id = input.item_id().to_owned();
        let guidance = input.take().guidance().to_vec();
        let finalized = input.take().finalized();
        let epoch = self.begin_interim()?;
        self.last_interim_end = Some(window.end);
        let (samples, samples_owner) = window.samples.into_decode();
        self.interim_task = Some(tokio::spawn(async move {
            let request = DecodeRequest::new(DecodeMode::Interim, samples, guidance, finalized)
                .with_lifetime_guard(samples_owner);
            let transcript = engine.decode(request).await;
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
        let transcript = match transcript {
            Ok(transcript) => transcript,
            Err(TranscribeError::Overloaded { .. }) => {
                tracing::debug!(
                    item_id = %item_id,
                    audio_start_ms = sample_millis(audio_start),
                    audio_end_ms = sample_millis(audio_end),
                    "skipped an interim tick: the transcription worker queue is full"
                );
                // The full worker queue never decoded this window, so the
                // next tick may submit it again.
                self.last_interim_end = None;
                return Ok(None);
            }
            Err(error) => return Err(SessionError::Inference(error)),
        };
        if transcript.text().is_empty() {
            return Ok(None);
        }
        let Some((snapshot, finalized)) = input.take().next_window_snapshot(
            transcript.text(),
            transcript.word_ends(),
            segment_start,
            audio_start,
            audio_end,
        ) else {
            return Ok(None);
        };
        self.take_update(item_id, snapshot, finalized, Some((audio_start, audio_end)))
    }

    /// The update for final outcomes that landed since the take's last
    /// update, so finalized text reaches the client while interim decodes
    /// are skipped as silent, come back empty, or are rejected.
    pub(crate) fn finalized_update(&mut self) -> Result<Option<ServerEvent>, SessionError> {
        let Some(input) = self.input.as_ref() else {
            return Ok(None);
        };
        let Some((snapshot, finalized)) = input.take().refreshed_snapshot(self.shown_finalized_seq)
        else {
            return Ok(None);
        };
        let item_id = input.item_id().to_owned();
        self.take_update(item_id, snapshot, finalized, None)
    }

    /// Emits `snapshot` as a hypothesis, or as a plain client's delta.
    /// `window` is the interim window decoded for it; a hypothesis equal to
    /// the last one keeps its revision and is sent again for a new window.
    /// An update without one reports the empty span at the latest window's
    /// end and is sent only when it changes the hypothesis; before any
    /// hypothesis, an empty snapshot counts as unchanged.
    fn take_update(
        &mut self,
        item_id: String,
        snapshot: InterimSnapshot,
        finalized: FinalizedRange,
        window: Option<(u64, u64)>,
    ) -> Result<Option<ServerEvent>, SessionError> {
        self.shown_finalized_seq = finalized.seq;
        if let Some((_, end)) = window {
            self.hypothesis_window_end = end;
        }
        let input = self.input.as_ref().ok_or(SessionError::NoInput)?;
        let include_ranges = input.snapshot().include_ranges();
        if !input.snapshot().include_hypothesis() {
            return Ok(self.standard_delta(item_id, snapshot));
        }
        let shown = (
            snapshot,
            include_ranges.then(|| HypothesisRanges {
                finalized_through_ms: sample_millis(finalized.through_samples),
                finalized_seq: finalized.seq,
            }),
        );
        let changed = self
            .last_hypothesis
            .as_ref()
            .map_or(!shown.0.is_empty(), |last| last != &shown);
        if changed {
            self.hypothesis_revision = self
                .hypothesis_revision
                .checked_add(1)
                .ok_or(SessionError::EpochExhausted)?;
            self.last_hypothesis = Some(shown.clone());
        } else if window.is_none() || self.last_hypothesis.is_none() {
            return Ok(None);
        }
        let (snapshot, ranges) = shown;
        let end = self.hypothesis_window_end;
        let (audio_start, audio_end) = window.unwrap_or((end, end));
        Ok(Some(ServerEvent::hypothesis(
            self.ids.event(),
            item_id,
            self.hypothesis_revision,
            snapshot,
            sample_millis(audio_start),
            sample_millis(audio_end),
            ranges,
        )))
    }

    fn standard_delta(
        &mut self,
        item_id: String,
        snapshot: InterimSnapshot,
    ) -> Option<ServerEvent> {
        let (_, finalized, agreed, _) = snapshot.into_parts();
        let stable_agreed_end = token_spans(&agreed)
            .into_iter()
            .rev()
            .nth(STANDARD_HOLD_BACK_WORDS)
            .map_or(0, |(_, _, end)| end);
        let stable_len = finalized.len() + stable_agreed_end;
        self.standard_interim_committed = finalized + &agreed;
        let delta = self.standard_interim_committed[..stable_len]
            .strip_prefix(self.standard_interim_sent.as_str())
            .filter(|delta| !delta.is_empty())?
            .to_owned();
        self.standard_interim_sent.push_str(&delta);
        Some(ServerEvent::transcription_delta(
            self.ids.event(),
            item_id,
            delta,
        ))
    }

    pub(crate) fn take_pending_interim(&mut self, item_id: &str) -> Option<ServerEvent> {
        self.hypothesis_revision = 0;
        self.last_hypothesis = None;
        self.shown_finalized_seq = 0;
        self.hypothesis_window_end = 0;
        let sent = std::mem::take(&mut self.standard_interim_sent);
        let committed = std::mem::take(&mut self.standard_interim_committed);
        let rest = committed
            .strip_prefix(sent.as_str())
            .filter(|rest| !rest.is_empty())?;
        Some(ServerEvent::transcription_delta(
            self.ids.event(),
            item_id.to_owned(),
            rest.to_owned(),
        ))
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
#[path = "route-tests.rs"]
mod tests;
