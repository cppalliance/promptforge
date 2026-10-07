//! Interim transcript snapshot split into finalized, agreed, and tentative
//! parts, and the finalized range behind its finalized part.

/// Absolute sample watermark of finalized text and the count of final
/// outcomes applied to the take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FinalizedRange {
    pub(crate) through_samples: u64,
    pub(crate) seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterimSnapshot {
    transcript: String,
    finalized: String,
    agreed: String,
    tentative: String,
}

impl InterimSnapshot {
    pub(super) fn new(finalized: String, agreed: String, tentative: String) -> Self {
        let transcript = format!("{finalized}{agreed}{tentative}");
        Self {
            transcript,
            finalized,
            agreed,
            tentative,
        }
    }

    pub(crate) fn committed(&self) -> &str {
        &self.transcript[..self.finalized.len() + self.agreed.len()]
    }

    pub(crate) fn into_parts(self) -> (String, String, String, String) {
        (self.transcript, self.finalized, self.agreed, self.tentative)
    }
}
