//! Whole-window interim state that merges hypotheses with the live prefix.

use std::ops::Range;

use super::agreement::{equivalent_token, matching_token_prefix_end, token_spans};
use super::interim::InterimSnapshot;
use super::live_prefix::LivePrefixSnapshot;
use super::text::append_transcript;

const MAX_PENDING_ACCEPTED_HYPOTHESES: usize = 2_048;
const MIN_LEADING_REPLACEMENT_TOKENS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AcceptedHypothesisCapacity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AcceptedHypothesis {
    range: Range<u64>,
    text: String,
}

impl AcceptedHypothesis {
    pub(super) fn new(range: Range<u64>, text: String) -> Self {
        Self { range, text }
    }

    pub(super) fn range(&self) -> Range<u64> {
        self.range.clone()
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Default)]
pub(super) struct WholeWindowState {
    segment_start: u64,
    window_start: Option<u64>,
    active: String,
    active_range: Option<Range<u64>>,
    pending: Vec<AcceptedHypothesis>,
    last: Option<InterimSnapshot>,
}

impl WholeWindowState {
    #[cfg(test)]
    fn next(
        &mut self,
        finalized: &str,
        finalized_samples: u64,
        segment_start: u64,
        window_start: u64,
        window_end: u64,
        hypothesis: &str,
    ) -> Option<InterimSnapshot> {
        let live_prefix = LivePrefixSnapshot::for_test(finalized, finalized_samples, None);
        self.try_next(
            &live_prefix,
            segment_start,
            window_start,
            window_end,
            hypothesis,
        )
        .unwrap_or(None)
    }

    pub(super) fn try_next(
        &mut self,
        live_prefix: &LivePrefixSnapshot,
        segment_start: u64,
        window_start: u64,
        window_end: u64,
        hypothesis: &str,
    ) -> Result<Option<InterimSnapshot>, AcceptedHypothesisCapacity> {
        let coverage_end = live_prefix.coverage_end();
        self.pending
            .retain(|accepted| accepted.range.start >= coverage_end);
        if self
            .active_range
            .as_ref()
            .is_some_and(|range| range.start < coverage_end)
        {
            self.active.clear();
            self.active_range = None;
            self.window_start = None;
        }
        if self.window_start.is_some() && self.segment_start != segment_start {
            self.finish_active_region(coverage_end)?;
            self.window_start = None;
        }
        self.segment_start = segment_start;

        let (replacement, active_start) = match self.window_start {
            Some(_) if self.active.is_empty() => (hypothesis.to_owned(), window_start),
            Some(_)
                if self
                    .active_range
                    .as_ref()
                    .is_some_and(|range| window_start >= range.end) =>
            {
                self.finish_active_region(coverage_end)?;
                (hypothesis.to_owned(), window_start)
            }
            Some(previous_start) if window_start > previous_start => {
                let Some(replacement) = rebase_sliding_window(&self.active, hypothesis) else {
                    return Ok(None);
                };
                let active_start = self
                    .active_range
                    .as_ref()
                    .map_or(window_start, |range| range.start);
                (replacement, active_start)
            }
            Some(_) | None => (hypothesis.to_owned(), window_start),
        };
        let agreed_end = if self.active.is_empty() {
            0
        } else {
            matching_token_prefix_end(&self.active, &replacement)
        };
        self.active = replacement;
        self.active_range = Some(active_start..window_end);
        self.window_start = Some(window_start);

        let mut agreed = String::new();
        if let Some((pending_forced, _)) = live_prefix.pending_forced() {
            append_transcript(&mut agreed, pending_forced);
        }
        for accepted in &self.pending {
            append_transcript(&mut agreed, accepted.text());
        }
        append_transcript(&mut agreed, self.active[..agreed_end].trim());
        let agreed = owned_piece(!live_prefix.finalized().is_empty(), &agreed);
        let tentative = owned_piece(
            !live_prefix.finalized().is_empty() || !agreed.is_empty(),
            &self.active[agreed_end..],
        );
        let snapshot = InterimSnapshot::new(live_prefix.finalized().to_owned(), agreed, tentative);
        if self.last.as_ref() == Some(&snapshot) {
            return Ok((!hypothesis.is_empty()).then_some(snapshot));
        }
        self.last = Some(snapshot.clone());
        Ok(Some(snapshot))
    }

    pub(super) fn accepted_hypotheses(&self, committed_samples: u64) -> Vec<AcceptedHypothesis> {
        let mut accepted = self
            .pending
            .iter()
            .filter(|hypothesis| hypothesis.range.end <= committed_samples)
            .cloned()
            .collect::<Vec<_>>();
        if !self.active.is_empty()
            && let Some(range) = &self.active_range
            && range.end <= committed_samples
        {
            accepted.push(AcceptedHypothesis::new(range.clone(), self.active.clone()));
        }
        accepted
    }

    #[cfg(feature = "test-fixtures")]
    pub(super) fn retained_hypothesis_count(&self) -> usize {
        self.pending.len() + usize::from(!self.active.is_empty())
    }

    fn finish_active_region(
        &mut self,
        finalized_samples: u64,
    ) -> Result<(), AcceptedHypothesisCapacity> {
        let range = self.active_range.take();
        if !self.active.is_empty()
            && let Some(range) = range
            && range.start >= finalized_samples
        {
            if self.pending.len() + 1 >= MAX_PENDING_ACCEPTED_HYPOTHESES {
                self.active_range = Some(range);
                return Err(AcceptedHypothesisCapacity);
            }
            self.pending.push(AcceptedHypothesis::new(
                range,
                std::mem::take(&mut self.active),
            ));
        }
        self.active.clear();
        Ok(())
    }
}

fn rebase_sliding_window(previous: &str, current: &str) -> Option<String> {
    let previous_tokens = token_spans(previous);
    let current_tokens = token_spans(current);
    for overlap in (1..=previous_tokens.len().min(current_tokens.len())).rev() {
        let previous_start = previous_tokens.len() - overlap;
        if previous_tokens[previous_start..]
            .iter()
            .map(|(token, _, _)| *token)
            .zip(current_tokens[..overlap].iter().map(|(token, _, _)| *token))
            .all(|(previous, current)| equivalent_token(previous, current))
        {
            let mut rebased = previous[..previous_tokens[previous_start].1]
                .trim_end()
                .to_owned();
            append_transcript(&mut rebased, current);
            return Some(rebased);
        }
    }
    if previous_tokens.len() >= MIN_LEADING_REPLACEMENT_TOKENS
        && current_tokens.len() >= MIN_LEADING_REPLACEMENT_TOKENS
        && previous_tokens
            .iter()
            .zip(&current_tokens)
            .take(MIN_LEADING_REPLACEMENT_TOKENS)
            .all(|((previous, _, _), (current, _, _))| {
                previous.chars().any(char::is_alphanumeric) && equivalent_token(previous, current)
            })
    {
        return Some(current.to_owned());
    }
    None
}

fn owned_piece(has_prefix: bool, piece: &str) -> String {
    if !has_prefix || piece.is_empty() || piece.starts_with(char::is_whitespace) {
        piece.to_owned()
    } else {
        format!(" {piece}")
    }
}

#[cfg(test)]
#[path = "window-tests-live-prefix.rs"]
mod live_prefix_tests;

#[cfg(test)]
#[path = "window-tests.rs"]
mod tests;
