use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SegmentOutcome {
    Decode(Range<u64>),
    Forced(ForcedBoundary),
    Skipped(Range<u64>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ForcedBoundary {
    decode_range: Range<u64>,
    new_audio: Range<u64>,
    overlap: Option<Range<u64>>,
    retain_overlap: bool,
}

impl ForcedBoundary {
    pub(crate) fn first(new_audio: Range<u64>) -> Self {
        Self {
            decode_range: new_audio.clone(),
            new_audio,
            overlap: None,
            retain_overlap: true,
        }
    }

    pub(crate) fn overlapping(overlap: Range<u64>, new_audio: Range<u64>) -> Self {
        Self::overlapping_successor(overlap, new_audio, true)
    }

    pub(super) fn overlapping_final(overlap: Range<u64>, new_audio: Range<u64>) -> Self {
        Self::overlapping_successor(overlap, new_audio, false)
    }

    fn overlapping_successor(
        overlap: Range<u64>,
        new_audio: Range<u64>,
        retain_overlap: bool,
    ) -> Self {
        debug_assert_eq!(overlap.end, new_audio.start);
        Self {
            decode_range: overlap.start..new_audio.end,
            new_audio,
            overlap: Some(overlap),
            retain_overlap,
        }
    }

    pub(crate) fn decode_range(&self) -> Range<u64> {
        self.decode_range.clone()
    }

    pub(crate) fn new_audio(&self) -> Range<u64> {
        self.new_audio.clone()
    }

    pub(crate) fn overlap(&self) -> Option<Range<u64>> {
        self.overlap.clone()
    }

    pub(crate) const fn retains_overlap(&self) -> bool {
        self.retain_overlap
    }
}
