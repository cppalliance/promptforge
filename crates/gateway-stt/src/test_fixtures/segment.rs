//! Deterministic segmentation fixtures.

/// Returns every closed speech range produced by the service segmenter.
#[must_use]
pub fn segment_ranges(samples: &[f32]) -> Vec<std::ops::Range<u64>> {
    let mut segmenter = crate::segment::Segmenter::new();
    let mut ranges = Vec::new();
    while let Some(outcome) = segmenter.poll(samples, 0) {
        match outcome {
            crate::segment::SegmentOutcome::Decode(range) => ranges.push(range),
            crate::segment::SegmentOutcome::Forced(boundary) => {
                ranges.push(boundary.decode_range());
            }
            crate::segment::SegmentOutcome::Skipped(_) => {}
        }
    }
    ranges
}
