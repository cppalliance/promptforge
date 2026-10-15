//! Test support: seeded pseudo-random input, serde checks, and the
//! auto-trait check.

use std::fmt::{Debug, Display};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::seed::RunSeed;

/// The seed every draw in this crate's tests starts from; a failure names it.
pub(crate) const SEED: u64 = 0x5EED;

/// A `SplitMix64` generator over a fixed seed, so a randomized test replays
/// exactly. Its checks put `seed <n>` in every failure message.
pub(crate) struct Seeded {
    seed: u64,
    state: u64,
}

impl Seeded {
    pub(crate) fn new(seed: u64) -> Self {
        Self { seed, state: seed }
    }

    pub(crate) fn seed(&self) -> u64 {
        self.seed
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let z = (self.state ^ (self.state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A run seed of four draws.
    pub(crate) fn run_seed(&mut self) -> RunSeed {
        let mut bytes = [0u8; 32];
        for chunk in bytes.as_chunks_mut::<8>().0 {
            *chunk = self.next_u64().to_le_bytes();
        }
        RunSeed::from_bytes(bytes)
    }

    /// A draw in `0..n`.
    #[track_caller]
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let draw = self.next_u64() % u64::try_from(n).expect("a usize fits in a u64");
        usize::try_from(draw).expect("a draw below a usize is a usize")
    }

    #[track_caller]
    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    #[track_caller]
    pub(crate) fn check(&self, condition: bool, message: impl Display) {
        assert!(condition, "{message} (seed {})", self.seed);
    }

    #[track_caller]
    pub(crate) fn check_eq<T: PartialEq + Debug>(
        &self,
        left: &T,
        right: &T,
        message: impl Display,
    ) {
        assert_eq!(left, right, "{message} (seed {})", self.seed);
    }
}

/// `value` serialized to JSON text and read back.
#[track_caller]
pub(crate) fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    let line = serde_json::to_string(value).expect("the value serializes");
    serde_json::from_str(&line).expect("the value's own line reads back")
}

/// Asserts that `value` serializes to exactly `line`, field order included,
/// and that `line` reads back as `value`.
#[track_caller]
pub(crate) fn assert_wire<T>(value: &T, line: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let actual = serde_json::to_string(value).expect("the value serializes");
    assert_eq!(actual, line);
    let back: T = serde_json::from_str(line).expect("the line reads back");
    assert_eq!(&back, value);
}

/// Compiles only for a type that can cross threads and live as long as the
/// program.
pub(crate) const fn assert_send_sync<T: Send + Sync + 'static>() {}

#[test]
fn seeded_failure_messages_name_the_seed() {
    let rng = Seeded::new(0x5EED);
    let seed = format!("seed {}", rng.seed());
    let failures = [
        std::panic::catch_unwind(|| rng.check(false, "a property failed")),
        std::panic::catch_unwind(|| rng.check_eq(&1, &2, "a count differed")),
    ];
    for failure in failures {
        let payload = failure.expect_err("a failing check panics");
        let message = payload.downcast_ref::<String>().expect("a message");
        assert!(message.contains(&seed), "{message:?} names no {seed}");
    }
}
