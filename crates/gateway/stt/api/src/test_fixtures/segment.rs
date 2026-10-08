//! Deterministic segmentation fixtures.

use gateway_stt_engine::EnergyDetector;

use crate::segment::{SegmentOutcome, Segmenter};

/// Returns every closed speech range produced by the service segmenter.
#[must_use]
pub fn segment_ranges(samples: &[f32]) -> Vec<std::ops::Range<u64>> {
    let mut segmenter = Segmenter::new(Box::new(EnergyDetector));
    segmenter
        .classify(samples, 0, true)
        .unwrap_or_else(|_| unreachable!("loudness classifies every frame"));
    let mut ranges = Vec::new();
    while let Some(outcome) = segmenter.poll() {
        match outcome {
            SegmentOutcome::Decode(range) => ranges.push(range),
            SegmentOutcome::Forced(boundary) => ranges.push(boundary.decode_range()),
            SegmentOutcome::Skipped(_) => {}
        }
    }
    ranges
}
