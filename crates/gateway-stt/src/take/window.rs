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

#[derive(Debug)]
struct PendingRegion {
    end: usize,
    accepted: AcceptedHypothesis,
}

#[derive(Debug, Default)]
pub(super) struct WholeWindowState {
    segment_start: usize,
    window_start: Option<usize>,
    active: String,
    active_end: usize,
    pending: Vec<PendingRegion>,
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
        self.pending.retain(|region| region.end > finalized_samples);
        if self.window_start.is_some() && self.segment_start != segment_start {
            if !self.active.is_empty() {
                self.pending.push(PendingRegion {
                    end: segment_start,
                    accepted: AcceptedHypothesis::new(
                        self.segment_start..self.active_end,
                        std::mem::take(&mut self.active),
                    ),
                });
            }
            self.window_start = None;
        }
        self.segment_start = segment_start;

        let replacement = match self.window_start {
            Some(previous_start) if window_start > previous_start => {
                rebase_sliding_window(&self.active, hypothesis)
            }
            Some(_) | None => hypothesis.to_owned(),
        };
        let agreed_end = if self.active.is_empty() {
            0
        } else {
            matching_token_prefix_end(&self.active, &replacement)
        };
        self.active = replacement;
        self.active_end = window_end;
        self.window_start = Some(window_start);

        let mut agreed = String::new();
        for region in &self.pending {
            append_transcript(&mut agreed, region.accepted.text());
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
            .map(|region| region.accepted.clone())
            .filter(|hypothesis| hypothesis.range.end <= committed_samples)
            .collect::<Vec<_>>();
        if !self.active.is_empty() && self.active_end <= committed_samples {
            accepted.push(AcceptedHypothesis::new(
                self.segment_start..self.active_end,
                self.active.clone(),
            ));
        }
        accepted
    }
}

fn rebase_sliding_window(previous: &str, current: &str) -> String {
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
            return rebased;
        }
    }
    current.to_owned()
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
