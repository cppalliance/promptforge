use std::ops::Range;

use super::agreement::{matching_token_prefix_end, token_spans};
use super::interim::InterimSnapshot;
use super::text::append_transcript;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AcceptedHypothesis {
    range: Range<usize>,
    text: String,
}

impl AcceptedHypothesis {
    pub(super) fn new(range: Range<usize>, text: String) -> Self {
        Self { range, text }
    }

    pub(super) fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Default)]
pub(super) struct WholeWindowState {
    segment_start: usize,
    window_start: Option<usize>,
    active: String,
    active_range: Option<Range<usize>>,
    pending: Vec<AcceptedHypothesis>,
    last: Option<InterimSnapshot>,
}

impl WholeWindowState {
    pub(super) fn next(
        &mut self,
        finalized: &str,
        finalized_samples: usize,
        segment_start: usize,
        window_start: usize,
        window_end: usize,
        hypothesis: &str,
    ) -> Option<InterimSnapshot> {
        self.pending
            .retain(|accepted| accepted.range.start >= finalized_samples);
        if self.window_start.is_some() && self.segment_start != segment_start {
            self.finish_active_region(finalized_samples);
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
                self.finish_active_region(finalized_samples);
                (hypothesis.to_owned(), window_start)
            }
            Some(previous_start) if window_start > previous_start => {
                let replacement = rebase_sliding_window(&self.active, hypothesis)?;
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
        for accepted in &self.pending {
            append_transcript(&mut agreed, accepted.text());
        }
        append_transcript(&mut agreed, self.active[..agreed_end].trim());
        let agreed = owned_piece(!finalized.is_empty(), &agreed);
        let tentative = owned_piece(
            !finalized.is_empty() || !agreed.is_empty(),
            &self.active[agreed_end..],
        );
        let snapshot = InterimSnapshot::new(finalized.to_owned(), agreed, tentative);
        if self.last.as_ref() == Some(&snapshot) {
            return (!hypothesis.is_empty()).then_some(snapshot);
        }
        self.last = Some(snapshot.clone());
        Some(snapshot)
    }

    pub(super) fn accepted_hypotheses(&self, committed_samples: usize) -> Vec<AcceptedHypothesis> {
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

    fn finish_active_region(&mut self, finalized_samples: usize) {
        let range = self.active_range.take();
        if !self.active.is_empty()
            && let Some(range) = range
            && range.start >= finalized_samples
        {
            self.pending.push(AcceptedHypothesis::new(
                range,
                std::mem::take(&mut self.active),
            ));
        }
        self.active.clear();
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
    None
}

fn equivalent_token(left: &str, right: &str) -> bool {
    left.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .eq(right
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase))
}

fn owned_piece(has_prefix: bool, piece: &str) -> String {
    if !has_prefix || piece.is_empty() || piece.starts_with(char::is_whitespace) {
        piece.to_owned()
    } else {
        format!(" {piece}")
    }
}

#[cfg(test)]
mod tests {
    use super::WholeWindowState;
    use crate::take::final_outcome::{FinalRangeOutcome, SkipReason, assemble_completion};

    #[test]
    fn whole_window_revision_replaces_a_promoted_leading_phrase() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 8_000, "Why is it");
        state.next("", 0, 0, 0, 9_600, "Why is it");
        let snapshot = state
            .next("", 0, 0, 0, 11_200, "Why is this")
            .expect("a revised whole-window hypothesis emits");

        assert_eq!(
            snapshot.into_parts(),
            (
                "Why is this".to_owned(),
                String::new(),
                "Why is".to_owned(),
                " this".to_owned(),
            )
        );
    }

    #[test]
    fn consumed_boundary_starts_a_region_before_finalization_arrives() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "first segment");
        state.next("", 0, 0, 0, 16_000, "first segment");
        let pending = state
            .next("", 0, 16_000, 16_000, 24_000, "second start")
            .expect("the new segment starts without waiting for final text");
        assert_eq!(pending.into_parts().0, "first segment second start");

        let authoritative = state
            .next(
                "revised first",
                16_000,
                16_000,
                16_000,
                25_600,
                "second start now",
            )
            .expect("authoritative text replaces the pending segment");
        assert_eq!(
            authoritative.into_parts().0,
            "revised first second start now"
        );
    }

    #[test]
    fn advancing_window_rebases_through_overlap_without_repeating_it() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "ask not what your country can do");
        let snapshot = state
            .next("", 0, 0, 8_000, 24_000, "your country can do for you")
            .expect("the sliding window emits a rebased hypothesis");

        assert_eq!(
            snapshot.into_parts().0,
            "ask not what your country can do for you"
        );
    }

    #[test]
    fn no_overlap_waits_for_disjoint_audio_before_appending_once() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 128_000, "alpha beta");
        assert!(state.next("", 0, 0, 8_000, 136_000, "gamma").is_none());
        assert!(state.next("", 0, 0, 16_000, 144_000, "delta").is_none());
        let snapshot = state
            .next("", 0, 0, 160_000, 256_000, "gamma delta")
            .expect("disjoint audio appends one new region");
        assert_eq!(snapshot.into_parts().0, "alpha beta gamma delta");

        let revision = state
            .next("", 0, 0, 176_000, 264_000, "delta epsilon")
            .expect("the new region keeps ordinary overlap replacement");
        assert_eq!(revision.into_parts().0, "alpha beta gamma delta epsilon");

        let accepted = state.accepted_hypotheses(264_000);
        assert_eq!(accepted.len(), 2);
        assert_eq!(accepted[0].range(), 0..128_000);
        assert_eq!(accepted[0].text(), "alpha beta");
        assert_eq!(accepted[1].range(), 160_000..264_000);
        assert_eq!(accepted[1].text(), "gamma delta epsilon");
    }

    #[test]
    fn finalized_boundary_does_not_retain_the_superseded_region() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "earlier words");
        let snapshot = state
            .next(
                "revised earlier",
                16_000,
                48_000,
                48_000,
                56_000,
                "new words",
            )
            .expect("finalized ownership replaces the earlier region");
        assert_eq!(snapshot.into_parts().0, "revised earlier new words");
    }

    #[test]
    fn finalized_watermark_crossing_active_retires_the_whole_hypothesis() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 8_000, 8_000, 24_000, "crossing words");
        let snapshot = state
            .next("authoritative", 16_000, 32_000, 32_000, 40_000, "new words")
            .expect("the finalized crossing emits its replacement");

        assert_eq!(snapshot.into_parts().0, "authoritative new words");
        let accepted = state.accepted_hypotheses(40_000);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].range(), 32_000..40_000);
        assert_eq!(accepted[0].text(), "new words");
    }

    #[test]
    fn active_starting_at_finalized_watermark_remains_owned() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 16_000, 16_000, 24_000, "boundary words");
        let snapshot = state
            .next("authoritative", 16_000, 32_000, 32_000, 40_000, "new words")
            .expect("text at the exact watermark remains live");

        assert_eq!(
            snapshot.into_parts().0,
            "authoritative boundary words new words"
        );
        let accepted = state.accepted_hypotheses(40_000);
        assert_eq!(accepted.len(), 2);
        assert_eq!(accepted[0].range(), 16_000..24_000);
        assert_eq!(accepted[1].range(), 32_000..40_000);
    }

    #[test]
    fn repeated_phrase_in_a_disjoint_region_is_appended_once() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "echo now");
        let first = state
            .next("", 0, 0, 32_000, 48_000, "echo now")
            .expect("the repeated phrase has disjoint audio ownership");
        assert_eq!(first.into_parts().0, "echo now echo now");

        let revision = state
            .next("", 0, 0, 32_000, 56_000, "echo now please")
            .expect("the repeated region is replaced instead of appended");
        assert_eq!(revision.into_parts().0, "echo now echo now please");
    }

    #[test]
    fn decoded_then_skipped_completion_uses_only_the_disjoint_candidate() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 128_000, "draft first");
        state.next("", 0, 0, 160_000, 256_000, "accepted tail");
        let accepted = state.accepted_hypotheses(256_000);
        let outcomes = [
            FinalRangeOutcome::decoded(0..160_000, "decoded first".to_owned()),
            FinalRangeOutcome::skipped(160_000..256_000, SkipReason::Silence),
        ];

        assert_eq!(
            assemble_completion(&outcomes, &accepted, 256_000),
            "decoded first accepted tail"
        );
    }

    #[test]
    fn sliding_overlap_tolerates_native_punctuation_revision() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 64_000, "And so my fellow Americans, ask");
        let snapshot = state
            .next("", 0, 0, 16_000, 80_000, "my fellow Americans ask not")
            .expect("the punctuated native overlap rebases");

        assert_eq!(
            snapshot.into_parts().0,
            "And so my fellow Americans ask not"
        );
    }

    #[test]
    fn accepted_hypothesis_retains_exact_committed_audio_coverage() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 32_000, 32_000, 40_000, "old");
        state.next("", 0, 32_000, 32_000, 44_800, "last word");

        assert!(
            state.accepted_hypotheses(40_000).is_empty(),
            "a snapshot extending beyond committed audio is not reusable"
        );
        let accepted = state.accepted_hypotheses(44_800);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].range(), 32_000..44_800);
        assert_eq!(accepted[0].text(), "last word");
    }
}
