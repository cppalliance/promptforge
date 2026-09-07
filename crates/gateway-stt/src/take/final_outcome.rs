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
    pub(super) range: Range<usize>,
    pub(super) result: FinalRangeResult,
}

impl FinalRangeOutcome {
    pub(super) fn decoded(range: Range<usize>, text: String) -> Self {
        Self {
            range,
            result: FinalRangeResult::Decoded(text),
        }
    }

    pub(super) fn skipped(range: Range<usize>, reason: SkipReason) -> Self {
        Self {
            range,
            result: FinalRangeResult::Skipped(reason),
        }
    }
}

pub(super) fn assemble_completion(
    outcomes: &[FinalRangeOutcome],
    accepted: &[AcceptedHypothesis],
    committed_samples: usize,
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
    hypothesis: &Range<usize>,
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
