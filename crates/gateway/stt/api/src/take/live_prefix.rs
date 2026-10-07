//! Snapshot of the finalized live prefix and any pending forced text.

use std::ops::Range;

use super::interim::FinalizedRange;

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingForcedSnapshot {
    text: String,
    range: Range<u64>,
}

/// Displayed words a natural final left after its last word, of which those
/// ending by byte `agreed_end` were shown agreed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AnchoredSuffix {
    text: String,
    agreed_end: usize,
}

impl AnchoredSuffix {
    pub(super) const fn new(text: String, agreed_end: usize) -> Self {
        Self { text, agreed_end }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) const fn agreed_end(&self) -> usize {
        self.agreed_end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LivePrefixSnapshot {
    finalized: String,
    finalized_samples: u64,
    applied_outcomes: u64,
    pending_forced: Option<PendingForcedSnapshot>,
    anchored: Option<AnchoredSuffix>,
}

impl LivePrefixSnapshot {
    pub(super) fn new(
        finalized: String,
        finalized_samples: u64,
        applied_outcomes: u64,
        pending_forced: Option<(String, Range<u64>)>,
        anchored: Option<AnchoredSuffix>,
    ) -> Self {
        Self {
            finalized,
            finalized_samples,
            applied_outcomes,
            pending_forced: pending_forced
                .map(|(text, range)| PendingForcedSnapshot { text, range }),
            anchored,
        }
    }

    pub(super) fn finalized(&self) -> &str {
        &self.finalized
    }

    pub(super) const fn finalized_range(&self) -> FinalizedRange {
        FinalizedRange {
            through_samples: self.finalized_samples,
            seq: self.applied_outcomes,
        }
    }

    /// Displayed words the latest natural final left after its last word.
    pub(super) const fn anchored(&self) -> Option<&AnchoredSuffix> {
        self.anchored.as_ref()
    }

    #[cfg(test)]
    pub(super) const fn finalized_samples(&self) -> u64 {
        self.finalized_samples
    }

    pub(super) fn pending_forced(&self) -> Option<(&str, Range<u64>)> {
        self.pending_forced
            .as_ref()
            .map(|pending| (pending.text.as_str(), pending.range.clone()))
    }

    pub(super) fn coverage_end(&self) -> u64 {
        self.pending_forced
            .as_ref()
            .map_or(self.finalized_samples, |pending| pending.range.end)
    }

    #[cfg(test)]
    pub(super) fn for_test(
        finalized: &str,
        finalized_samples: u64,
        pending_forced: Option<(&str, Range<u64>)>,
    ) -> Self {
        Self::new(
            finalized.to_owned(),
            finalized_samples,
            0,
            pending_forced.map(|(text, range)| (text.to_owned(), range)),
            None,
        )
    }
}
