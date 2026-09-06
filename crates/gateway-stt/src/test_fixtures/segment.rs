//! Deterministic segmentation fixtures.

/// Returns every closed speech range produced by the service segmenter.
#[must_use]
pub fn segment_ranges(samples: &[f32]) -> Vec<std::ops::Range<usize>> {
    let mut segmenter = crate::segment::Segmenter::new();
    let mut ranges = Vec::new();
    while let Some(range) = segmenter.poll(samples) {
        ranges.push(range);
    }
    ranges
}
