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
        ),
        first.find("Then the silver bird")
    );
    assert_eq!(
        range_guided_suffix_prefix_start(
            second,
            32_000..320_000,
            third,
            192_000..480_000,
            192_000..320_000,
        ),
        second.find("quiet garden")
    );
}

#[test]
fn bounded_alignment_accepts_reviewed_edits_and_rejects_excess() {
    let previous = "settled one, two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
    let accepted = "one two three extra four five SIX seven nine ten eleven dozen thirteen fourteen fifteen fresh";
    let rejected = "one changed three extra four five six seven nine ten eleven dozen thirteen fourteen fifteen fresh";

    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..180, accepted, 20..200, 20..180),
        previous.find("one,")
    );
    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..180, rejected, 20..200, 20..180),
        None,
        "four edits across fifteen aligned tokens exceed the reviewed ratio"
    );
}

#[test]
fn clearly_nearer_repeated_chorus_is_not_ambiguous() {
    let previous = "intro chorus one two chorus one two";
    let current = "chorus one two chorus one two outro";

    assert_eq!(
        range_guided_suffix_prefix_start(previous, 0..100, current, 60..160, 60..100),
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
    assert_eq!(alignment, None);
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
    assert_eq!(alignment, None);
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
    assert_eq!(alignment, None);
    assert_eq!(metrics.tokens, 1);
    assert_eq!((metrics.normalization_work, metrics.dp_cells), (0, 0));
}

#[test]
fn accepted_alignment_reports_every_bounded_work_dimension() {
    let previous = "settled one two three four five six seven eight nine ten eleven twelve";
    let current = "one two three four five six seven eight nine ten eleven twelve fresh alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
    let (alignment, metrics) =
        range_guided_suffix_prefix_start_with_metrics(previous, 0..160, current, 32..320, 32..160);

    assert_eq!(alignment, previous.find("one"));
    assert_eq!(metrics.input_bytes, previous.len() + current.len());
    assert!(metrics.tokens <= MAX_CONSIDERED_TOKENS * 2);
    assert!(metrics.normalization_work <= MAX_NORMALIZATION_WORK);
    assert!(metrics.dp_cells <= MAX_ALIGNMENT_DP_CELLS);
    assert!(metrics.normalization_work > 0 && metrics.dp_cells > 0);
}
