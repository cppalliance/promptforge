//! Gateway reconnect backoff: jittered exponential probe delays that
//! reset only on useful work, never on mere connect.
//!
//! One [`ReconnectBackoff`] is shared between the heartbeat (which draws
//! a delay before each probe while the gateway is unreachable) and the
//! chat paths (which record useful work - a delivered streaming token or
//! a successful buffered completion). A gateway that connects but never
//! delivers keeps escalating: answering the health probe is not useful
//! work, so a flapping upstream cannot ride the connect/disconnect cycle
//! back to the fast schedule (rqbit's anti-flap discipline). The delays
//! are jittered so workshops restarted together do not probe in phase,
//! and a total-delay budget bounds the retry campaign as a whole: once
//! the cumulative delay handed out since the last useful work crosses
//! it, [`ReconnectBackoff::next_delay`] answers `None` and the caller
//! stops reconnecting.

use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// First retry delay while the gateway is unreachable, matching the
/// steady-state heartbeat cadence so the first retry is no slower than
/// the probing it replaces.
const BASE_DELAY: Duration = Duration::from_secs(5);

/// Ceiling on a single retry delay. A returning gateway is noticed
/// within a minute even after a long outage; everything in the UI hangs
/// off the heartbeat's verdict, so the ceiling stays desktop-friendly
/// rather than adopting rqbit's one-hour swarm value.
const MAX_DELAY: Duration = Duration::from_secs(60);

/// Total delay the backoff may hand out between two pieces of useful
/// work before it gives up - a budget instead of an attempt cap, per
/// rqbit. At the 60s ceiling this is more than a day of continuous
/// outage; a workshop whose gateway has been gone that long stops
/// probing and says so.
const TOTAL_DELAY_BUDGET: Duration = Duration::from_secs(24 * 60 * 60);

/// Shared reconnect-backoff state; clones feed one schedule.
///
/// The heartbeat calls [`ReconnectBackoff::next_delay`] before each
/// probe while the gateway reads unreachable; the chat paths call
/// [`ReconnectBackoff::record_useful_work`] when the gateway proves
/// itself. Nothing else mutates the schedule - in particular, a probe
/// that merely connects leaves it untouched.
#[derive(Debug, Clone)]
pub(crate) struct ReconnectBackoff {
    base: Duration,
    max: Duration,
    budget: Duration,
    state: Arc<Mutex<State>>,
}

/// The mutable half: where the schedule stands since the last useful work.
#[derive(Debug)]
struct State {
    /// The un-jittered delay the next `next_delay` call draws from;
    /// doubles per call up to the ceiling.
    current: Duration,
    /// Cumulative delay handed out since the last useful work, judged
    /// against the budget.
    spent: Duration,
    /// xorshift64 state for jitter; never zero (the generator's fixed
    /// point).
    rng: u64,
}

impl ReconnectBackoff {
    /// A backoff on the production schedule.
    pub(crate) fn new() -> Self {
        Self::with_schedule(BASE_DELAY, MAX_DELAY, TOTAL_DELAY_BUDGET)
    }

    /// A backoff on an explicit schedule, so tests run in milliseconds.
    pub(crate) fn with_schedule(base: Duration, max: Duration, budget: Duration) -> Self {
        debug_assert!(!base.is_zero(), "a zero base would hand out zero delays");
        // std's RandomState is randomly seeded per process, which is all
        // the entropy jitter needs; `| 1` dodges xorshift's zero fixed
        // point.
        let seed = std::hash::RandomState::new().build_hasher().finish() | 1;
        Self {
            base,
            max,
            budget,
            state: Arc::new(Mutex::new(State {
                current: base,
                spent: Duration::ZERO,
                rng: seed,
            })),
        }
    }

    /// The next reconnect delay: the current schedule step, jittered
    /// uniformly into its upper half (between half the step and the full
    /// step), with the step then doubled up to the ceiling. Answers
    /// `None` once the cumulative delay handed out since the last useful
    /// work has crossed the budget - the signal to stop reconnecting.
    pub(crate) fn next_delay(&self) -> Option<Duration> {
        let mut state = self.lock_state();
        if state.spent >= self.budget {
            return None;
        }
        let raw = state.current;
        let floor = raw / 2;
        // Delays are capped at the ceiling (a minute), so the span always
        // fits; the fallback only guards the arithmetic, and the
        // saturating add keeps even the u64::MAX fallback from overflowing.
        let span = u64::try_from(raw.saturating_sub(floor).as_nanos()).unwrap_or(u64::MAX);
        let jitter = if span == 0 {
            0
        } else {
            xorshift(&mut state.rng) % span.saturating_add(1)
        };
        let delay = floor + Duration::from_nanos(jitter);
        state.spent = state.spent.saturating_add(delay);
        state.current = raw.saturating_mul(2).min(self.max);
        Some(delay)
    }

    /// Resets the schedule to its base and refills the budget. Called
    /// only when the gateway does useful work - a delivered streaming
    /// token or a successful buffered completion - never on a probe that
    /// merely connects.
    pub(crate) fn record_useful_work(&self) {
        let mut state = self.lock_state();
        state.current = self.base;
        state.spent = Duration::ZERO;
    }

    /// Whether the schedule has escalated past its base since the last
    /// useful work.
    #[cfg(test)]
    pub(crate) fn is_escalated_for_test(&self) -> bool {
        self.lock_state().current > self.base
    }

    /// A peer that panicked while holding the lock cannot corrupt two
    /// plain `Duration`s, so the value is recovered rather than wedging
    /// every reconnect.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// xorshift64: a tiny deterministic generator; jitter needs spread, not
/// cryptography, and this keeps the dependency tree unchanged. Shared
/// with the gateway tests, which seed it explicitly so each randomized
/// failure names its seed.
pub(crate) fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schedule small enough to exhaust inside a test: base 10ms,
    /// ceiling 40ms, budget 200ms.
    fn fast() -> ReconnectBackoff {
        ReconnectBackoff::with_schedule(
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_millis(200),
        )
    }

    #[test]
    fn delays_escalate_within_jitter_bounds_and_clamp_at_the_ceiling() {
        let backoff = fast();
        // The un-jittered schedule is 10, 20, 40, 40, ... and each drawn
        // delay lands in the upper half of its step.
        for expected in [10u64, 20, 40, 40, 40] {
            let raw = Duration::from_millis(expected);
            let delay = backoff.next_delay().expect("the budget is not spent yet");
            assert!(
                delay >= raw / 2 && delay <= raw,
                "a delay of {delay:?} left the jitter window of the {raw:?} step"
            );
        }
    }

    #[test]
    fn the_budget_exhausts_to_none_and_stays_exhausted() {
        let backoff = fast();
        // Every delay is at least 5ms, so 200ms of budget is spent well
        // within 40 draws; the exact count depends on jitter.
        let exhausted_after = (0..40).find(|_| backoff.next_delay().is_none());
        assert!(
            exhausted_after.is_some(),
            "the budget must exhaust within the schedule's worst case"
        );
        assert_eq!(
            backoff.next_delay(),
            None,
            "an exhausted budget stays exhausted until useful work"
        );
    }

    #[test]
    fn useful_work_resets_the_schedule_and_refills_the_budget() {
        let backoff = fast();
        while backoff.next_delay().is_some() {}
        backoff.record_useful_work();
        assert!(
            !backoff.is_escalated_for_test(),
            "useful work returns the schedule to its base"
        );
        let delay = backoff
            .next_delay()
            .expect("useful work refills the budget");
        assert!(
            delay <= Duration::from_millis(10),
            "the first delay after a reset draws from the base step, got {delay:?}"
        );
    }

    #[test]
    fn clones_share_one_schedule() {
        let backoff = fast();
        let hook = backoff.clone();
        let _ = backoff.next_delay();
        assert!(backoff.is_escalated_for_test());
        hook.record_useful_work();
        assert!(
            !backoff.is_escalated_for_test(),
            "a reset through one clone reaches every holder"
        );
    }
}
