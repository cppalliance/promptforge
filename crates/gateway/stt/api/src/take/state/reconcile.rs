//! Reconciliation of final windows: the estimate for forced windows whose
//! overlap did not align, and the aligned rewrite of a natural final over the
//! interim text it replaces.

use std::ops::Range;

use super::FinalizedState;
use crate::take::agreement::{
    AlignmentFailure, anchored_final_end, normalized_token, projected_prefix_end, token_spans,
};
use crate::take::live_prefix::AnchoredSuffix;
use crate::take::text::append_transcript;
use crate::take::window::ShownHypotheses;

/// The divisor, 3, that defines a sparse successor: one whose words per sample
/// are under one third of its predecessor's dropped speech the predecessor
/// heard, rather than decoding the same speech differently.
const SPARSE_SUCCESSOR_DENSITY_DIVISOR: u128 = 3;

/// How a forced successor window settles its predecessor's pending text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Settlement {
    /// The predecessor settles its text before this byte and the successor's
    /// whole text becomes pending over its whole decode range.
    Prefix(usize),
    /// The successor decoded too few words for the audio it covers, so the
    /// predecessor settles whole and only the successor's text from this byte
    /// becomes pending, over the audio after the overlap.
    SparseSuccessor(usize),
}

/// Settles a forced successor whose overlap with its predecessor did not
/// align, logging the estimate it falls back to. `None` when the overlap
/// cannot be projected onto the predecessor.
pub(super) fn estimate(
    previous_text: &str,
    previous_range: &Range<u64>,
    text: &str,
    current_range: &Range<u64>,
    overlap: &Range<u64>,
    failure: &AlignmentFailure,
) -> Option<Settlement> {
    if successor_is_sparse(previous_text, previous_range, text, current_range)
        && let Some(kept) = projected_prefix_end(text, current_range.clone(), overlap.end)
    {
        tracing::warn!(
            warning_code = "forced_final_successor_sparse",
            alignment_failure = failure.reason.code(),
            prior_decode_start = previous_range.start,
            prior_decode_end = previous_range.end,
            current_decode_start = current_range.start,
            current_decode_end = current_range.end,
            overlap_start = overlap.start,
            overlap_end = overlap.end,
            prior_tokens = previous_text.split_whitespace().count(),
            current_tokens = kept.metrics.tokens,
            "sparse forced final successor, so its predecessor was kept whole"
        );
        return Some(Settlement::SparseSuccessor(kept.byte_end));
    }
    let projection = projected_prefix_end(previous_text, previous_range.clone(), overlap.start)?;
    tracing::warn!(
        warning_code = "forced_final_overlap_estimated",
        alignment_failure = failure.reason.code(),
        alignment_input_bytes = failure.metrics.input_bytes,
        alignment_tokens = failure.metrics.tokens,
        alignment_normalization_work = failure.metrics.normalization_work,
        alignment_dp_cells = failure.metrics.dp_cells,
        prior_decode_start = previous_range.start,
        prior_decode_end = previous_range.end,
        current_decode_start = current_range.start,
        current_decode_end = current_range.end,
        overlap_start = overlap.start,
        overlap_end = overlap.end,
        projection_input_bytes = projection.metrics.input_bytes,
        projection_tokens = projection.metrics.tokens,
        projection_audio_before_overlap = projection.metrics.audio_before_overlap,
        projection_audio_total = projection.metrics.audio_total,
        projection_rounded_tokens = projection.metrics.rounded_tokens,
        projection_selected_tokens = projection.metrics.selected_tokens,
        projection_punctuation_examined = projection.metrics.punctuation_examined,
        projection_punctuation_candidates = projection.metrics.punctuation_candidates,
        projection_rounding = "nearest_ties_earlier",
        "estimated forced final overlap reconciliation"
    );
    Some(Settlement::Prefix(projection.byte_end))
}

/// Whether the successor's words per sample are under a third of the
/// predecessor's, compared exactly without division.
fn successor_is_sparse(
    previous_text: &str,
    previous_range: &Range<u64>,
    text: &str,
    current_range: &Range<u64>,
) -> bool {
    let samples = |range: &Range<u64>| u128::from(range.end.saturating_sub(range.start));
    let (previous_samples, current_samples) = (samples(previous_range), samples(current_range));
    if previous_samples == 0 || current_samples == 0 {
        return false;
    }
    let previous_tokens = previous_text.split_whitespace().count() as u128;
    let current_tokens = text.split_whitespace().count() as u128;
    current_tokens * previous_samples * SPARSE_SUCCESSOR_DENSITY_DIVISOR
        < previous_tokens * current_samples
}

/// Displayed tokens before kept words that must equal the final's last tokens.
const SUFFIX_ANCHOR_TOKENS: usize = 2;
/// Displayed words after the final's last word that an anchored rewrite keeps.
const MAX_ANCHORED_SUFFIX_WORDS: usize = 5;

/// Settles `text` as the authoritative final through `range_end` and keeps
/// the displayed words after its last word live when they are anchored.
pub(super) fn rewrite_natural(
    state: &mut FinalizedState,
    text: &str,
    range_end: u64,
    shown: &ShownHypotheses,
) {
    let (displayed, agreed_end) = shown.after(state.samples);
    append_transcript(&mut state.text, text);
    append_transcript(&mut state.decoded_text, text);
    state.samples = range_end;
    state.transcribed_samples = range_end;
    state.anchored = anchored_final_end(text, &displayed, SUFFIX_ANCHOR_TOKENS)
        .and_then(|end| anchored_suffix(&displayed, end, agreed_end))
        .filter(|suffix| !repeats_final(text, suffix.text()));
}

/// Whether every word of `suffix`, in order, repeats a run of words in
/// `final_text`: the fast pass echoing audio the final covers, which it
/// often decodes from trailing silence, rather than speech after the final.
fn repeats_final(final_text: &str, suffix: &str) -> bool {
    let words = |text: &str| {
        token_spans(text)
            .into_iter()
            .map(|(token, _, _)| normalized_token(token))
            .collect::<Vec<_>>()
    };
    let echoed = words(suffix);
    !echoed.is_empty()
        && words(final_text)
            .windows(echoed.len())
            .any(|run| run == echoed.as_slice())
}

/// The first `MAX_ANCHORED_SUFFIX_WORDS` words of `displayed` after byte
/// `end`, of which those ending by byte `agreed_end` were shown agreed.
fn anchored_suffix(displayed: &str, end: usize, agreed_end: usize) -> Option<AnchoredSuffix> {
    let mut text = String::new();
    let mut agreed = 0;
    for (word, _, word_end) in token_spans(&displayed[end..])
        .into_iter()
        .take(MAX_ANCHORED_SUFFIX_WORDS)
    {
        append_transcript(&mut text, word);
        if end + word_end <= agreed_end {
            agreed = text.len();
        }
    }
    (!text.is_empty()).then(|| AnchoredSuffix::new(text, agreed))
}
