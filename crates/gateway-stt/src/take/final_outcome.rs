use std::ops::Range;

use super::text::append_transcript;
use super::window::AcceptedHypothesis;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SkipReason {
    BelowSpeechThreshold,
    BelowFinalWindow,
    Silence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FinalRangeResult {
    Decoded(String),
    Skipped(SkipReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FinalRangeOutcome {
    pub(super) range: Range<u64>,
    pub(super) result: FinalRangeResult,
}

impl FinalRangeOutcome {
    pub(super) fn decoded(range: Range<u64>, text: String) -> Self {
        Self {
            range,
            result: FinalRangeResult::Decoded(text),
        }
    }

    pub(super) fn skipped(range: Range<u64>, reason: SkipReason) -> Self {
        Self {
            range,
            result: FinalRangeResult::Skipped(reason),
        }
    }
}

pub(super) fn assemble_completion(
    outcomes: &[FinalRangeOutcome],
    accepted: &[AcceptedHypothesis],
    committed_samples: u64,
) -> String {
    let mut transcript = String::new();
    let mut used = vec![false; accepted.len()];
    for (outcome_index, outcome) in outcomes.iter().enumerate() {
        match &outcome.result {
            FinalRangeResult::Decoded(text) => append_transcript(&mut transcript, text),
            FinalRangeResult::Skipped(_) => {
                let candidate = accepted.iter().enumerate().find(|(index, hypothesis)| {
                    !used[*index]
                        && hypothesis.range().end <= committed_samples
                        && skipped_outcomes_exactly_cover(
                            outcomes,
                            outcome_index,
                            &hypothesis.range(),
                        )
                });
                if let Some((index, hypothesis)) = candidate {
                    used[index] = true;
                    append_transcript(&mut transcript, hypothesis.text());
                }
            }
        }
    }
    transcript
}

fn skipped_outcomes_exactly_cover(
    outcomes: &[FinalRangeOutcome],
    first: usize,
    hypothesis: &Range<u64>,
) -> bool {
    if outcomes[first].range.start > hypothesis.start
        || outcomes[first].range.end <= hypothesis.start
    {
        return false;
    }
    let mut covered_end = hypothesis.start;
    for outcome in &outcomes[first..] {
        if !matches!(outcome.result, FinalRangeResult::Skipped(_)) {
            return false;
        }
        if outcome.range.end <= covered_end {
            continue;
        }
        if outcome.range.start > covered_end {
            return false;
        }
        covered_end = outcome.range.end;
        if covered_end >= hypothesis.end {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{FinalRangeOutcome, SkipReason, assemble_completion};
    use crate::take::window::AcceptedHypothesis;

    #[test]
    fn final_outcomes_and_accepted_text_keep_absolute_ranges() {
        let origin = u64::from(u32::MAX) + 32_000;
        let outcomes = [
            FinalRangeOutcome::decoded(origin..origin + 16_000, "first".to_owned()),
            FinalRangeOutcome::skipped(
                origin + 16_000..origin + 24_000,
                SkipReason::BelowFinalWindow,
            ),
        ];
        let accepted = [AcceptedHypothesis::new(
            origin + 16_000..origin + 24_000,
            "tail".to_owned(),
        )];

        let range: std::ops::Range<u64> = outcomes[1].range.clone();
        assert_eq!(
            assemble_completion(&outcomes, &accepted, origin + 24_000),
            "first tail"
        );
        assert_eq!(range.start, origin + 16_000);
    }
}
