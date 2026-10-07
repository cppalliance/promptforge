//! Whole-window interim state that merges hypotheses with the live prefix.

use std::ops::Range;

use super::agreement::{equivalent_token, normalized_token, token_spans};
use super::interim::InterimSnapshot;
use super::live_prefix::LivePrefixSnapshot;
use super::text::append_transcript;

mod evidence;

use evidence::{Agreement, covering_prefix};

const MAX_PENDING_ACCEPTED_HYPOTHESES: usize = 2_048;
const MIN_LEADING_REPLACEMENT_TOKENS: usize = 2;
/// Inverse of the largest share of an anchored suffix's aligned tokens that
/// a fast pass may change and still re-derive the suffix.
const REDERIVED_EDIT_DENOMINATOR: usize = 2;

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

/// The hypotheses a window renders after its live prefix: the pending ones,
/// shown agreed, then the active one regardless of its range end, with the
/// byte end of its agreed text.
#[derive(Clone, Debug, Default)]
pub(super) struct ShownHypotheses {
    pending: Vec<AcceptedHypothesis>,
    active: Option<(AcceptedHypothesis, usize)>,
}

impl ShownHypotheses {
    /// Text shown for audio after sample `samples` and the byte end of its
    /// agreed prefix.
    pub(super) fn after(&self, samples: u64) -> (String, usize) {
        let mut text = String::new();
        for pending in self
            .pending
            .iter()
            .filter(|pending| pending.range.end > samples)
        {
            append_transcript(&mut text, pending.text());
        }
        let Some((active, agreed_end)) = self
            .active
            .as_ref()
            .filter(|(active, _)| active.range.end > samples)
        else {
            let agreed = text.len();
            return (text, agreed);
        };
        append_transcript(&mut text, active.text[..*agreed_end].trim());
        let agreed = text.len();
        append_transcript(&mut text, active.text[*agreed_end..].trim());
        (text, agreed)
    }
}

#[derive(Debug, Default)]
pub(super) struct WholeWindowState {
    segment_start: u64,
    window_start: Option<u64>,
    active: String,
    active_range: Option<Range<u64>>,
    agreement: Agreement,
    pending: Vec<AcceptedHypothesis>,
    last: Option<InterimSnapshot>,
    /// Coverage end of the live prefix whose anchored suffix seeded the
    /// active text, so each suffix seeds it once.
    seeded_through: Option<u64>,
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

        let (replacement, active_start) =
            if self.seed_anchored_suffix(live_prefix, window_start, window_end) {
                (with_anchored_suffix(&self.active, hypothesis), window_start)
            } else {
                match self.window_start {
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
                        let Some(replacement) = rebase_sliding_window(&self.active, hypothesis)
                        else {
                            return Ok(None);
                        };
                        let active_start = self
                            .active_range
                            .as_ref()
                            .map_or(window_start, |range| range.start);
                        (replacement, active_start)
                    }
                    Some(_) | None => (hypothesis.to_owned(), window_start),
                }
            };
        if self.active.is_empty() {
            self.agreement = Agreement::default();
        }
        let Some(active) =
            self.agreement
                .revise(&self.active, hypothesis, &replacement, window_end)
        else {
            return Ok(None);
        };
        let agreed_end = self.agreement.agreed_end();
        self.active = active;
        self.active_range = Some(active_start..window_end);
        self.window_start = Some(window_start);

        let snapshot = compose(live_prefix, &self.pending, &self.active, agreed_end);
        if self.last.as_ref() == Some(&snapshot) {
            return Ok((!hypothesis.is_empty()).then_some(snapshot));
        }
        self.last = Some(snapshot.clone());
        Ok(Some(snapshot))
    }

    /// Recomposes the shown snapshot over `live_prefix` after final outcomes
    /// landed, without a new hypothesis. Text that starts before the live
    /// prefix's text end is hidden, since settled or pending forced text holds
    /// its words, but stays accepted for the skipped ranges that may need it.
    /// The anchored suffix a natural final left shows in its place until the
    /// next hypothesis seeds from it.
    pub(super) fn refresh(&mut self, live_prefix: &LivePrefixSnapshot) -> InterimSnapshot {
        let text_end = live_prefix.text_end();
        let pending = self
            .pending
            .iter()
            .filter(|accepted| accepted.range.start >= text_end)
            .collect::<Vec<_>>();
        let active_shown = !self.active.is_empty()
            && self
                .active_range
                .as_ref()
                .is_some_and(|range| range.start >= text_end);
        let (active, agreed_end) = if active_shown {
            (self.active.as_str(), self.agreement.agreed_end())
        } else {
            live_prefix
                .anchored()
                .filter(|_| {
                    pending.is_empty()
                        && live_prefix.pending_forced().is_none()
                        && self.seeded_through != Some(live_prefix.coverage_end())
                })
                .map_or(("", 0), |suffix| (suffix.text(), suffix.agreed_end()))
        };
        let snapshot = compose(live_prefix, pending, active, agreed_end);
        self.last = Some(snapshot.clone());
        snapshot
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

    pub(super) fn shown(&self) -> ShownHypotheses {
        ShownHypotheses {
            pending: self.pending.clone(),
            active: self
                .active_range
                .clone()
                .filter(|_| !self.active.is_empty())
                .map(|range| {
                    (
                        AcceptedHypothesis::new(range, self.active.clone()),
                        self.agreement.agreed_end(),
                    )
                }),
        }
    }

    /// Starts the active text over `window_start..window_end` from the live
    /// prefix's anchored suffix, keeping its agreed words agreed, when nothing
    /// else follows the live prefix and the suffix has not seeded it before.
    fn seed_anchored_suffix(
        &mut self,
        live_prefix: &LivePrefixSnapshot,
        window_start: u64,
        window_end: u64,
    ) -> bool {
        let coverage_end = live_prefix.coverage_end();
        let Some(suffix) = live_prefix.anchored() else {
            return false;
        };
        if !self.active.is_empty()
            || !self.pending.is_empty()
            || live_prefix.pending_forced().is_some()
            || self.seeded_through == Some(coverage_end)
        {
            return false;
        }
        self.seeded_through = Some(coverage_end);
        suffix.text().clone_into(&mut self.active);
        self.active_range = Some(window_start..window_end);
        self.window_start = Some(window_start);
        self.agreement = Agreement::seeded(suffix.text(), suffix.agreed_end());
        true
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

/// Merges `hypothesis`, the first fast pass after a natural final, onto
/// `suffix`, the displayed words that final left after its last word: the
/// hypothesis replaces the suffix words it re-derives and follows the rest.
fn with_anchored_suffix(suffix: &str, hypothesis: &str) -> String {
    if let Some(rebased) = rebase_sliding_window(suffix, hypothesis) {
        return rebased;
    }
    if rederives(suffix, hypothesis) {
        return hypothesis.to_owned();
    }
    let mut text = suffix.to_owned();
    append_transcript(&mut text, hypothesis.trim_start());
    text
}

/// Whether the leading tokens of `hypothesis` align with every token of
/// `suffix`, changing at most `1 / REDERIVED_EDIT_DENOMINATOR` of them.
fn rederives(suffix: &str, hypothesis: &str) -> bool {
    let suffix = normalized_tokens(suffix);
    let suffix = suffix.iter().map(String::as_str).collect::<Vec<_>>();
    covering_prefix(&suffix, &normalized_tokens(hypothesis)).is_some_and(|(edits, length)| {
        length > 0 && edits * REDERIVED_EDIT_DENOMINATOR <= suffix.len().max(length)
    })
}

fn normalized_tokens(text: &str) -> Vec<String> {
    token_spans(text)
        .into_iter()
        .map(|(token, _, _)| normalized_token(token))
        .collect()
}

/// The snapshot of `live_prefix`, the `pending` hypotheses, and `active`,
/// whose first `agreed_end` bytes are agreed.
fn compose<'pending>(
    live_prefix: &LivePrefixSnapshot,
    pending: impl IntoIterator<Item = &'pending AcceptedHypothesis>,
    active: &str,
    agreed_end: usize,
) -> InterimSnapshot {
    let mut agreed = String::new();
    if let Some((pending_forced, _)) = live_prefix.pending_forced() {
        append_transcript(&mut agreed, pending_forced);
    }
    for accepted in pending {
        append_transcript(&mut agreed, accepted.text());
    }
    append_transcript(&mut agreed, active[..agreed_end].trim());
    let agreed = owned_piece(!live_prefix.finalized().is_empty(), &agreed);
    let tentative = owned_piece(
        !live_prefix.finalized().is_empty() || !agreed.is_empty(),
        &active[agreed_end..],
    );
    InterimSnapshot::new(live_prefix.finalized().to_owned(), agreed, tentative)
}

fn owned_piece(has_prefix: bool, piece: &str) -> String {
    if !has_prefix || piece.is_empty() || piece.starts_with(char::is_whitespace) {
        piece.to_owned()
    } else {
        format!(" {piece}")
    }
}

#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod live_prefix_tests;
#[cfg(test)]
mod tests;
