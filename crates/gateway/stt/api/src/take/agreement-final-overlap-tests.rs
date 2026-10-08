//! Tests for range-guided suffix-prefix alignment of overlapping final windows.

use super::*;

#[test]
fn installed_cuda_whisper_windows_align_at_the_known_ranges() {
    let first = "The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances.";
    let second = "Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances.";
    let third = "quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window";

    assert_eq!(
        range_guided_suffix_prefix_start(
            first,
            0..160_000,
            second,
            32_000..320_000,
            32_000..160_000,
        )
        .ok(),
        first.find("Then the silver bird")
    );
    assert_eq!(
        range_guided_suffix_prefix_start(
            second,
            32_000..320_000,
            third,
            192_000..480_000,
            192_000..320_000,
        )
        .ok(),
        second.find("quiet garden")
    );
}

#[test]
fn bounded_alignment_accepts_reviewed_edits_and_rejects_excess() {
    let previous = "settled one, two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
    let accepted = "one two three extra four five SIX seven nine ten eleven dozen thirteen fourteen fifteen fresh";
    let rejected = "one changed three extra four five six seven nine ten eleven dozen thirteen fourteen fifteen fresh";

    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..180, accepted, 20..200, 20..180).ok(),
        previous.find("one,")
    );
    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..180, rejected, 20..200, 20..180)
            .map_err(|failure| failure.reason),
        Err(AlignmentReason::NoCandidate),
        "four edits across fifteen aligned tokens exceed the reviewed ratio"
    );
}

#[test]
fn clearly_nearer_repeated_chorus_is_not_ambiguous() {
    let previous = "intro chorus one two chorus one two";
    let current = "chorus one two chorus one two outro";

    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..100, current, 60..160, 60..100).ok(),
        previous.rfind("chorus")
    );
}

#[test]
fn preprocessing_rejects_bytes_tokens_and_huge_tokens_before_normalization() {
    let oversized = "x".repeat(MAX_FINAL_TRANSCRIPT_BYTES + 1);
    let (alignment, metrics) = range_guided_suffix_prefix_start_with_metrics(
        &oversized,
        0..160,
        "current",
        32..320,
        32..160,
    );
    assert_eq!(alignment, Err(AlignmentReason::TranscriptBytes));
    assert_eq!(metrics.input_bytes, oversized.len() + "current".len());
    assert_eq!(
        (metrics.tokens, metrics.normalization_work, metrics.dp_cells),
        (0, 0, 0)
    );

    let tiny = std::iter::repeat_n("x", 1_000)
        .collect::<Vec<_>>()
        .join(" ");
    let (alignment, metrics) =
        range_guided_suffix_prefix_start_with_metrics(&tiny, 0..160, &tiny, 32..320, 32..160);
    assert_eq!(alignment, Err(AlignmentReason::TokenLimit));
    assert!(metrics.tokens <= MAX_CONSIDERED_TOKENS + 1);
    assert_eq!((metrics.normalization_work, metrics.dp_cells), (0, 0));

    let huge_token = "x".repeat(MAX_NORMALIZED_TOKEN_BYTES + 1);
    let (alignment, metrics) = range_guided_suffix_prefix_start_with_metrics(
        &huge_token,
        0..160,
        "current",
        32..320,
        32..160,
    );
    assert_eq!(alignment, Err(AlignmentReason::TokenLimit));
    assert_eq!(metrics.tokens, 1);
    assert_eq!((metrics.normalization_work, metrics.dp_cells), (0, 0));
}

#[test]
fn accepted_alignment_reports_every_bounded_work_dimension() {
    let previous = "settled one two three four five six seven eight nine ten eleven twelve";
    let current = "one two three four five six seven eight nine ten eleven twelve fresh alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
    let (alignment, metrics) =
        range_guided_suffix_prefix_start_with_metrics(previous, 0..160, current, 32..320, 32..160);

    assert_eq!(alignment.ok(), previous.find("one"));
    assert_eq!(metrics.input_bytes, previous.len() + current.len());
    assert!(metrics.tokens <= MAX_CONSIDERED_TOKENS * 2);
    assert!(metrics.normalization_work <= MAX_NORMALIZATION_WORK);
    assert!(metrics.dp_cells <= MAX_ALIGNMENT_DP_CELLS);
    assert!(metrics.normalization_work > 0 && metrics.dp_cells > 0);
}

fn failure_reason(
    previous: &str,
    previous_range: Range<u64>,
    current: &str,
    current_range: Range<u64>,
    overlap: Range<u64>,
) -> AlignmentReason {
    range_guided_suffix_prefix_start(previous, previous_range, current, current_range, overlap)
        .expect_err("alignment fails")
        .reason
}

#[test]
fn an_over_limit_transcript_fails_with_the_byte_reason_on_either_side() {
    let oversized = "x ".repeat(MAX_FINAL_TRANSCRIPT_BYTES / 2 + 1);

    assert_eq!(
        failure_reason(&oversized, 0..160, "current", 32..320, 32..160),
        AlignmentReason::TranscriptBytes
    );
    assert_eq!(
        failure_reason("previous", 0..160, &oversized, 32..320, 32..160),
        AlignmentReason::TranscriptBytes
    );
}

#[test]
fn too_many_tokens_or_one_huge_token_fails_with_the_token_reason() {
    let many = "x ".repeat(MAX_CONSIDERED_TOKENS + 1);
    let huge = "x".repeat(MAX_NORMALIZED_TOKEN_BYTES + 1);

    assert_eq!(
        failure_reason(&many, 0..160, "current", 32..320, 32..160),
        AlignmentReason::TokenLimit
    );
    assert_eq!(
        failure_reason("previous", 0..160, &huge, 32..320, 32..160),
        AlignmentReason::TokenLimit
    );
}

#[test]
fn a_token_that_grows_past_the_bound_when_lowercased_fails_with_the_normalization_reason() {
    // Each U+0130 is 2 bytes and lowercases to 3, so 64 of them pass the
    // 128-byte raw token check but normalize to 192 bytes.
    let growing = "\u{130}".repeat(64);
    assert_eq!(growing.len(), MAX_NORMALIZED_TOKEN_BYTES);

    assert_eq!(
        failure_reason(&growing, 0..160, "current", 32..320, 32..160),
        AlignmentReason::NormalizationLimit
    );
    assert_eq!(
        failure_reason("previous", 0..160, &growing, 32..320, 32..160),
        AlignmentReason::NormalizationLimit
    );
}

#[test]
fn an_overlap_outside_either_decode_range_fails_with_the_range_reason() {
    assert_eq!(
        failure_reason("one two", 0..160, "one two", 32..320, 32..400),
        AlignmentReason::OverlapOutsideRange,
        "the overlap ends after the successor's decode range"
    );
    assert_eq!(
        failure_reason("one two", 64..160, "one two", 32..320, 32..160),
        AlignmentReason::OverlapOutsideRange,
        "the overlap starts before the predecessor's decode range"
    );
    assert_eq!(
        failure_reason("one two", 0..0, "one two", 32..320, 32..160),
        AlignmentReason::OverlapOutsideRange,
        "an empty predecessor range cannot project the overlap"
    );
}

#[test]
fn unrelated_text_over_a_sound_overlap_fails_with_the_no_candidate_reason() {
    let failure = range_guided_suffix_prefix_start(
        "alpha bravo charlie delta echo foxtrot golf hotel",
        0..160,
        "one two three four five six seven eight",
        32..320,
        32..160,
    )
    .expect_err("unrelated text does not align");

    assert_eq!(failure.reason, AlignmentReason::NoCandidate);
    assert!(
        failure.metrics.normalization_work > 0,
        "the failure carries the work done before it"
    );
}

#[test]
fn two_equally_good_repeated_choruses_fail_with_the_ambiguous_reason() {
    assert_eq!(
        failure_reason(
            "intro chorus one two three chorus one two three",
            0..90,
            "chorus one two three chorus one two three outro",
            30..120,
            30..90,
        ),
        AlignmentReason::AmbiguousCandidates
    );
}
