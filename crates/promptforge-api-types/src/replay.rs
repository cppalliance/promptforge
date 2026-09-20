//! Replay vocabulary: the behavior flags a run records and the two ways a
//! replay can fail.
//!
//! A run is meant to be reproducible from its log: the same run inputs
//! (seed, `started_at`, flags) and the same answers replayed in order
//! produce the same effects and events, each keyed by its
//! [`Provenance`](crate::ids::Provenance). Replay itself is
//! not built yet; these types are defined now so the log schema and the run
//! record have their columns from the first run written.
//!
//! [`Flags`] is how a future engine change that alters a recorded run's
//! behavior stays replayable: it runs the new behavior live and sets its
//! flag, and a later replay honors the flag only if the original run
//! recorded it. [`ReplayError`] keeps "the code under replay diverged" apart
//! from "the record is broken", because each demands a different remedy.

use std::ops::{BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "replay-tests.rs"]
mod tests;

/// The behavior flags recorded with a run: a bitset that is exactly one
/// `u32` on the wire and in the run record.
///
/// Numbering is reserve-forever: each flag a future change introduces is an
/// associated constant `Flags(1 << n)` whose bit `n` is assigned once and
/// never reused or renumbered, even after the behavior it gated becomes
/// the only behavior. No flag is defined yet. Bits this build does not name
/// are preserved through [`from_bits`](Self::from_bits) and
/// [`bits`](Self::bits), so a record written by a newer engine keeps its
/// flags through an older reader.
///
/// # Examples
/// ```
/// use promptforge_api_types::replay::Flags;
///
/// let recorded = Flags::from_bits(0b101);
/// assert!(recorded.contains(Flags::from_bits(0b100)));
/// assert!(!recorded.contains(Flags::from_bits(0b010)));
/// assert!(Flags::EMPTY.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Flags(u32);

impl Flags {
    /// No flag set: every run this plan produces records this value.
    pub const EMPTY: Flags = Flags(0);

    /// The set whose bits are exactly `bits`, unknown bits included.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The set as its `u32` bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// True when no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every flag in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Flags {
    type Output = Flags;

    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

impl BitOrAssign for Flags {
    fn bitor_assign(&mut self, rhs: Flags) {
        self.0 |= rhs.0;
    }
}

/// Why a replay failed.
///
/// The two kinds are properties of different things. `Nondeterminism` is a
/// property of the code under replay: re-executed against its record, a run
/// or a task issued an effect or event that disagrees with what the record
/// holds at that [`Provenance`](crate::ids::Provenance), so the engine (or
/// the prompt) is not deterministic where it must be. `Fatal` is a property
/// of the record: the log is malformed or internally inconsistent (an
/// effect with two answers, a sequence gap, an unparseable payload), so
/// there is nothing sound to replay against.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// The re-executed run or task disagreed with its record.
    #[error("replay diverged from its record: {detail}")]
    Nondeterminism {
        /// Where and how the re-execution disagreed.
        detail: String,
    },
    /// The record itself is malformed or internally inconsistent.
    #[error("replay record is malformed: {detail}")]
    Fatal {
        /// What is wrong with the record.
        detail: String,
    },
}
