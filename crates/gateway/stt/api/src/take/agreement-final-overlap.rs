//! Bounded token alignment that locates where consecutive final windows overlap.

use std::cmp::Ordering;
use std::ops::{Range, RangeInclusive};

use super::{AlignmentFailure, AlignmentMetrics, AlignmentReason};

const ALIGNMENT_TOKEN_BAND: usize = 8;
const MAX_ALIGNMENT_TOKENS: usize = 96;
const MAX_CONSIDERED_TOKENS: usize = 256;
pub(super) const MAX_FINAL_TRANSCRIPT_BYTES: usize = 16 * 1024;
const MAX_NORMALIZED_TOKEN_BYTES: usize = 128;
const MAX_NORMALIZATION_WORK: usize = MAX_ALIGNMENT_TOKENS * 2 * MAX_NORMALIZED_TOKEN_BYTES;
const MAX_EDIT_RATIO_DENOMINATOR: usize = 5;
const MIN_COVERAGE_NUMERATOR: usize = 2;
const MIN_COVERAGE_DENOMINATOR: usize = 3;
const MAX_ALIGNMENT_DP_CELLS: usize = (ALIGNMENT_TOKEN_BAND * 2 + 1)
    * (ALIGNMENT_TOKEN_BAND * 2 + 1)
    * MAX_ALIGNMENT_TOKENS
    * MAX_ALIGNMENT_TOKENS;

#[derive(Debug)]
struct AlignmentToken {
    normalized: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug)]
struct Alignment {
    previous_start: usize,
    current_end: usize,
    edits: usize,
    aligned_tokens: usize,
    evidence_tokens: usize,
    proximity: usize,
}

#[derive(Clone, Copy, Debug)]
struct TranscriptShape {
    tokens: usize,
}

struct AlignmentSearch<'a> {
    previous_tokens: &'a [AlignmentToken],
    current_tokens: &'a [AlignmentToken],
    previous_first: usize,
    previous_candidates: RangeInclusive<usize>,
    current_candidates: RangeInclusive<usize>,
    previous_estimate: usize,
    current_estimate: usize,
    projected_previous: usize,
    projected_current: usize,
}

pub(in crate::take) const fn final_transcript_within_limit(text: &str) -> bool {
    text.len() <= MAX_FINAL_TRANSCRIPT_BYTES
}

/// Byte offset in `previous` where the overlap with `current` begins, found by
/// aligning the tail of `previous` with the head of `current`.
///
/// # Errors
/// Returns why no single alignment was found, with the work the attempt did.
pub(in crate::take) fn range_guided_suffix_prefix_start(
    previous: &str,
    previous_range: Range<u64>,
    current: &str,
    current_range: Range<u64>,
    overlap: Range<u64>,
) -> Result<usize, AlignmentFailure> {
    let (start, metrics) = range_guided_suffix_prefix_start_with_metrics(
        previous,
        previous_range,
        current,
        current_range,
        overlap,
    );
    start.map_err(|reason| AlignmentFailure { reason, metrics })
}

fn range_guided_suffix_prefix_start_with_metrics(
    previous: &str,
    previous_range: Range<u64>,
    current: &str,
    current_range: Range<u64>,
    overlap: Range<u64>,
) -> (Result<usize, AlignmentReason>, AlignmentMetrics) {
    let mut metrics = AlignmentMetrics {
        input_bytes: previous.len().saturating_add(current.len()),
        ..AlignmentMetrics::default()
    };
    if !final_transcript_within_limit(previous) || !final_transcript_within_limit(current) {
        return (Err(AlignmentReason::TranscriptBytes), metrics);
    }
    let Some(previous_shape) = scan_transcript(previous, &mut metrics) else {
        return (Err(AlignmentReason::TokenLimit), metrics);
    };
    let Some(current_shape) = scan_transcript(current, &mut metrics) else {
        return (Err(AlignmentReason::TokenLimit), metrics);
    };
    let Some(previous_estimate) =
        projected_boundary(overlap.start, &previous_range, previous_shape.tokens)
    else {
        return (Err(AlignmentReason::OverlapOutsideRange), metrics);
    };
    let Some(current_estimate) =
        projected_boundary(overlap.end, &current_range, current_shape.tokens)
    else {
        return (Err(AlignmentReason::OverlapOutsideRange), metrics);
    };
    let previous_candidates = candidate_boundaries(
        previous_estimate,
        0,
        previous_shape.tokens.saturating_sub(1),
    );
    let current_candidates = candidate_boundaries(current_estimate, 1, current_shape.tokens);
    let previous_first = (*previous_candidates.start())
        .max(previous_shape.tokens.saturating_sub(MAX_ALIGNMENT_TOKENS));
    let current_last = (*current_candidates.end()).min(MAX_ALIGNMENT_TOKENS);
    let Some(previous_tokens) = collect_tokens(
        previous,
        previous_first..previous_shape.tokens,
        &mut metrics,
    ) else {
        return (Err(AlignmentReason::NormalizationLimit), metrics);
    };
    let Some(current_tokens) = collect_tokens(current, 0..current_last, &mut metrics) else {
        return (Err(AlignmentReason::NormalizationLimit), metrics);
    };
    let search = AlignmentSearch {
        previous_tokens: &previous_tokens,
        current_tokens: &current_tokens,
        previous_first,
        previous_candidates,
        current_candidates,
        previous_estimate,
        current_estimate,
        projected_previous: previous_shape.tokens.saturating_sub(previous_estimate),
        projected_current: current_estimate,
    };
    let mut candidates = search.candidates(&mut metrics);
    candidates.sort_by(|left, right| compare_alignment(*left, *right));
    let Some(best) = candidates.first().copied() else {
        return (Err(AlignmentReason::NoCandidate), metrics);
    };
    if candidates
        .iter()
        .skip(1)
        .copied()
        .any(|candidate| similarly_scored_distinct(best, candidate))
    {
        return (Err(AlignmentReason::AmbiguousCandidates), metrics);
    }
    (
        Ok(search.previous_tokens[best.previous_start - search.previous_first].start),
        metrics,
    )
}

/// Byte end in `displayed` of the token that the last token of `final_text`
/// aligns with, when the `anchor` displayed tokens ending there equal the
/// final's last `anchor` tokens.
///
/// Both texts start at the same audio, so the final's last tokens align with
/// the displayed tokens from the same index through an end within the token
/// band of the final's token count. The lowest edit ratio wins, then the end
/// nearest that count, then the later end.
pub(in crate::take) fn anchored_final_end(
    final_text: &str,
    displayed: &str,
    anchor: usize,
) -> Option<usize> {
    let mut metrics = AlignmentMetrics::default();
    if anchor == 0
        || !final_transcript_within_limit(final_text)
        || !final_transcript_within_limit(displayed)
    {
        return None;
    }
    let final_count = scan_transcript(final_text, &mut metrics)?.tokens;
    let displayed_count = scan_transcript(displayed, &mut metrics)?.tokens;
    let first = final_count.saturating_sub(MAX_ALIGNMENT_TOKENS);
    let ends = candidate_boundaries(final_count, first + anchor, displayed_count);
    if final_count < anchor || ends.is_empty() {
        return None;
    }
    let final_tokens = collect_tokens(final_text, first..final_count, &mut metrics)?;
    let displayed_tokens = collect_tokens(displayed, first..*ends.end(), &mut metrics)?;
    let mut best: Option<(usize, usize, usize)> = None;
    for end in ends {
        let Some(span) = displayed_tokens.get(..end - first) else {
            continue;
        };
        let aligned = span.len().max(final_tokens.len());
        let max_edits = aligned / MAX_EDIT_RATIO_DENOMINATOR;
        if span.len().abs_diff(final_tokens.len()) > max_edits {
            continue;
        }
        let Some(edits) = bounded_edit_distance(&final_tokens, span, max_edits, &mut metrics)
        else {
            continue;
        };
        if best.is_none_or(|(best_end, best_edits, best_aligned)| {
            (edits * best_aligned)
                .cmp(&(best_edits * aligned))
                .then_with(|| {
                    end.abs_diff(final_count)
                        .cmp(&best_end.abs_diff(final_count))
                })
                .then_with(|| best_end.cmp(&end))
                .is_lt()
        }) {
            best = Some((end, edits, aligned));
        }
    }
    let (end, _, _) = best?;
    let anchored = displayed_tokens.get(end - first - anchor..end - first)?;
    let tail = final_tokens.get(final_tokens.len().checked_sub(anchor)?..)?;
    if anchored
        .iter()
        .zip(tail)
        .all(|(shown, spoken)| shown.normalized == spoken.normalized)
    {
        anchored.last().map(|token| token.end)
    } else {
        None
    }
}

impl AlignmentSearch<'_> {
    fn candidates(&self, metrics: &mut AlignmentMetrics) -> Vec<Alignment> {
        let previous_tolerance = boundary_tolerance(self.projected_previous);
        let current_tolerance = boundary_tolerance(self.projected_current);
        let mut candidates = Vec::with_capacity((ALIGNMENT_TOKEN_BAND * 2 + 1).pow(2));
        for previous_start in self.previous_candidates.clone() {
            if previous_start < self.previous_first
                || previous_start.abs_diff(self.previous_estimate) > previous_tolerance
            {
                continue;
            }
            let previous_suffix = &self.previous_tokens[previous_start - self.previous_first..];
            for current_end in self.current_candidates.clone() {
                if current_end > self.current_tokens.len()
                    || current_end.abs_diff(self.current_estimate) > current_tolerance
                {
                    continue;
                }
                let current_prefix = &self.current_tokens[..current_end];
                let aligned_tokens = previous_suffix.len().max(current_prefix.len());
                if aligned_tokens == 0 || aligned_tokens > MAX_ALIGNMENT_TOKENS {
                    continue;
                }
                let max_edits = aligned_tokens / MAX_EDIT_RATIO_DENOMINATOR;
                if previous_suffix.len().abs_diff(current_prefix.len()) > max_edits {
                    continue;
                }
                let maximum_evidence = previous_suffix.len().min(current_prefix.len());
                if !covers_projected_overlap(
                    maximum_evidence,
                    self.projected_previous,
                    self.projected_current,
                ) {
                    continue;
                }
                let Some(edits) =
                    bounded_edit_distance(previous_suffix, current_prefix, max_edits, metrics)
                else {
                    continue;
                };
                let evidence_tokens = aligned_tokens.saturating_sub(edits);
                if !covers_projected_overlap(
                    evidence_tokens,
                    self.projected_previous,
                    self.projected_current,
                ) {
                    continue;
                }
                candidates.push(Alignment {
                    previous_start,
                    current_end,
                    edits,
                    aligned_tokens,
                    evidence_tokens,
                    proximity: previous_start.abs_diff(self.previous_estimate)
                        + current_end.abs_diff(self.current_estimate),
                });
            }
        }
        candidates
    }
}

fn scan_transcript(text: &str, metrics: &mut AlignmentMetrics) -> Option<TranscriptShape> {
    let mut tokens = 0;
    let mut start = None;
    let mut has_alphanumeric = false;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, character.is_whitespace()) {
            (None, false) => {
                start = Some(index);
                has_alphanumeric = character.is_alphanumeric();
            }
            (Some(_), false) => has_alphanumeric |= character.is_alphanumeric(),
            (Some(begin), true) => {
                metrics.tokens += usize::from(has_alphanumeric);
                if index.saturating_sub(begin) > MAX_NORMALIZED_TOKEN_BYTES {
                    return None;
                }
                if has_alphanumeric {
                    tokens += 1;
                    if tokens > MAX_CONSIDERED_TOKENS {
                        return None;
                    }
                }
                start = None;
                has_alphanumeric = false;
            }
            _ => {}
        }
    }
    Some(TranscriptShape { tokens })
}

fn collect_tokens(
    text: &str,
    selected: Range<usize>,
    metrics: &mut AlignmentMetrics,
) -> Option<Vec<AlignmentToken>> {
    let mut tokens = Vec::with_capacity(selected.len().min(MAX_ALIGNMENT_TOKENS));
    let mut token_index = 0;
    let mut start = None;
    let mut has_alphanumeric = false;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, character.is_whitespace()) {
            (None, false) => {
                start = Some(index);
                has_alphanumeric = character.is_alphanumeric();
            }
            (Some(_), false) => has_alphanumeric |= character.is_alphanumeric(),
            (Some(begin), true) => {
                if has_alphanumeric && selected.contains(&token_index) {
                    let normalized = normalize_token(&text[begin..index], metrics)?;
                    tokens.push(AlignmentToken {
                        normalized,
                        start: begin,
                        end: index,
                    });
                }
                if has_alphanumeric {
                    token_index += 1;
                }
                start = None;
                has_alphanumeric = false;
                if token_index >= selected.end {
                    break;
                }
            }
            _ => {}
        }
    }
    Some(tokens)
}

fn normalize_token(token: &str, metrics: &mut AlignmentMetrics) -> Option<String> {
    let mut normalized = String::with_capacity(token.len().min(MAX_NORMALIZED_TOKEN_BYTES));
    for character in token.chars() {
        metrics.normalization_work = metrics.normalization_work.saturating_add(1);
        if metrics.normalization_work > MAX_NORMALIZATION_WORK {
            return None;
        }
        if character.is_alphanumeric() {
            for lowercase in character.to_lowercase() {
                if normalized.len().saturating_add(lowercase.len_utf8())
                    > MAX_NORMALIZED_TOKEN_BYTES
                {
                    return None;
                }
                normalized.push(lowercase);
            }
        }
    }
    Some(normalized)
}

fn projected_boundary(point: u64, range: &Range<u64>, token_count: usize) -> Option<usize> {
    let length = range.end.checked_sub(range.start)?;
    let offset = point.checked_sub(range.start)?;
    if length == 0 || point > range.end {
        return None;
    }
    let numerator = u128::from(offset)
        .saturating_mul(token_count as u128)
        .saturating_add(u128::from(length / 2));
    usize::try_from(numerator / u128::from(length))
        .ok()
        .map(|boundary| boundary.min(token_count))
}

fn candidate_boundaries(estimate: usize, minimum: usize, maximum: usize) -> RangeInclusive<usize> {
    let start = estimate.saturating_sub(ALIGNMENT_TOKEN_BAND).max(minimum);
    let end = estimate.saturating_add(ALIGNMENT_TOKEN_BAND).min(maximum);
    start..=end
}

fn boundary_tolerance(projected_overlap_tokens: usize) -> usize {
    projected_overlap_tokens
        .div_ceil(4)
        .clamp(2, ALIGNMENT_TOKEN_BAND)
}

fn covers_projected_overlap(
    evidence_tokens: usize,
    projected_previous: usize,
    projected_current: usize,
) -> bool {
    evidence_tokens * MIN_COVERAGE_DENOMINATOR >= projected_previous * MIN_COVERAGE_NUMERATOR
        && evidence_tokens * MIN_COVERAGE_DENOMINATOR >= projected_current * MIN_COVERAGE_NUMERATOR
}

fn compare_alignment(left: Alignment, right: Alignment) -> Ordering {
    (left.edits * right.aligned_tokens)
        .cmp(&(right.edits * left.aligned_tokens))
        .then_with(|| left.proximity.cmp(&right.proximity))
        .then_with(|| right.evidence_tokens.cmp(&left.evidence_tokens))
        .then_with(|| right.aligned_tokens.cmp(&left.aligned_tokens))
        .then_with(|| left.previous_start.cmp(&right.previous_start))
        .then_with(|| left.current_end.cmp(&right.current_end))
}

fn similarly_scored_distinct(left: Alignment, right: Alignment) -> bool {
    let same_edit_ratio = left.edits * right.aligned_tokens == right.edits * left.aligned_tokens;
    same_edit_ratio
        && left.proximity.abs_diff(right.proximity) <= 1
        && (left.previous_start != right.previous_start || left.current_end != right.current_end)
}

fn bounded_edit_distance(
    left: &[AlignmentToken],
    right: &[AlignmentToken],
    max_edits: usize,
    metrics: &mut AlignmentMetrics,
) -> Option<usize> {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_token) in left.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_token) in right.iter().enumerate() {
            if metrics.dp_cells >= MAX_ALIGNMENT_DP_CELLS {
                return None;
            }
            metrics.dp_cells = metrics.dp_cells.saturating_add(1);
            let substitution = usize::from(left_token.normalized != right_token.normalized);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[right.len()] <= max_edits).then_some(previous[right.len()])
}

#[cfg(test)]
#[path = "agreement-final-overlap-tests.rs"]
mod tests;
