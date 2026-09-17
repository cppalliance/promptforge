use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingForcedSnapshot {
    text: String,
    range: Range<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LivePrefixSnapshot {
    finalized: String,
    finalized_samples: u64,
    pending_forced: Option<PendingForcedSnapshot>,
}

impl LivePrefixSnapshot {
    pub(super) fn new(
        finalized: String,
        finalized_samples: u64,
        pending_forced: Option<(String, Range<u64>)>,
    ) -> Self {
        Self {
            finalized,
            finalized_samples,
            pending_forced: pending_forced
                .map(|(text, range)| PendingForcedSnapshot { text, range }),
        }
    }

    pub(super) fn finalized(&self) -> &str {
        &self.finalized
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
            pending_forced.map(|(text, range)| (text.to_owned(), range)),
        )
    }
}
