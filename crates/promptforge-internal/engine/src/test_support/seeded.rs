//! Seeded pseudo-random test input, so a randomized test replays exactly
//! and every failure names the seed that produced it.

use std::fmt::{Debug, Display};

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

    /// Permutes `items` in place, Fisher-Yates over [`below`](Seeded::below).
    #[track_caller]
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for last in (1..items.len()).rev() {
            items.swap(last, self.below(last + 1));
        }
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

#[test]
fn seeded_failure_messages_name_the_seed() {
    let mut rng = Seeded::new(0x5EED);
    let mut replay = Seeded::new(rng.seed());
    let draw = rng.next_u64();
    rng.check_eq(&draw, &replay.next_u64(), "one seed replays its draws");
    let below = rng.below(3);
    rng.check(below < 3, "a draw below 3 is below 3");
    let items = ["a", "b", "c"];
    let picked = rng.pick(&items);
    rng.check(items.contains(picked), "a pick is one of the items");
    let mut shuffled = [1, 2, 3, 4];
    Seeded::new(rng.seed()).shuffle(&mut shuffled);
    let mut replayed = [1, 2, 3, 4];
    Seeded::new(rng.seed()).shuffle(&mut replayed);
    rng.check_eq(&shuffled, &replayed, "one seed replays its shuffle");
    rng.check(
        shuffled != [1, 2, 3, 4],
        "this seed's shuffle moves an item",
    );
    shuffled.sort_unstable();
    rng.check_eq(&shuffled, &[1, 2, 3, 4], "a shuffle is a permutation");
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
