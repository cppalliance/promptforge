//! The tokio driver's test-only surface: the seeded shuffle that holds
//! and permutes each wave of answers, the effect and round taps, and the
//! accessors the suites reach the run and its scheduler through.

use std::sync::{Arc, Mutex};

use crate::cancel::CancelHandle;
use crate::execute::scheduler::Scheduler;
use crate::execute::{Effect, EffectAnswer, EffectId, EffectRecord, Round, Run};

use super::TokioDriver;

impl TokioDriver<'_> {
    /// Test-only wait under a shuffle seed: holds the current wave's
    /// delivery until every outstanding effect has posted its answer,
    /// then delivers the whole wave in the seed's permutation. Holding
    /// the whole wave is what makes the shuffle exhaustive - answering
    /// one arrival at a time would rarely permute anything - and every
    /// performer posts independently of the run, so the hold cannot
    /// deadlock. The cancel flag still tears the run down from the
    /// hold.
    #[cfg(test)]
    pub(super) async fn await_shuffled_batch(&mut self) {
        let mut batch = Vec::new();
        loop {
            tokio::select! {
                biased;
                arrival = self.rx.recv() => {
                    if let Some(pair) = arrival {
                        batch.push(pair);
                        if batch.len() >= self.outstanding.len() {
                            self.deliver_batch(batch);
                            return;
                        }
                    } else {
                        self.deliver_batch(batch);
                        return;
                    }
                }
                () = self.cancel.cancelled() => {
                    self.run.cancel();
                    self.deliver_batch(batch);
                    return;
                }
            }
        }
    }

    /// Records every issued effect's record from here on, performed or
    /// dropped at issue.
    #[cfg(test)]
    pub(crate) fn record_effects_for_test(&mut self) -> Arc<Mutex<Vec<EffectRecord>>> {
        let tap = Arc::new(Mutex::new(Vec::new()));
        self.tap = Some(Arc::clone(&tap));
        tap
    }

    /// Records the round of every `Chat` effect issued from here on: its
    /// id, and its origin, which the effect's record leaves out.
    #[cfg(test)]
    pub(crate) fn record_rounds_for_test(&mut self) -> Arc<Mutex<Vec<Round>>> {
        let rounds = Arc::new(Mutex::new(Vec::new()));
        self.rounds = Some(Arc::clone(&rounds));
        rounds
    }

    /// Test-only: seeds the completion-order shuffle, so each seed
    /// yields one deterministic interleaving of concurrently completed
    /// effects.
    #[cfg(test)]
    pub(crate) fn set_shuffle_for_test(&mut self, seed: u64) {
        self.shuffle = Some(seed);
    }

    /// Appends one step's issued effects to the tap, and their rounds to
    /// the round tap, in issue order.
    #[cfg(test)]
    pub(super) fn record(
        &self,
        effects: &[(EffectId, promptforge_types::ids::Provenance, Effect)],
    ) {
        if let Some(tap) = &self.tap {
            tap.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(effects.iter().map(|(_, _, effect)| effect.record()));
        }
        if let Some(rounds) = &self.rounds {
            rounds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(effects.iter().filter_map(|(_, _, effect)| match effect {
                    Effect::Chat { round, .. } => Some(*round),
                    _ => None,
                }));
        }
    }

    /// The scheduler behind the run, for the suites that inspect its
    /// arena.
    #[cfg(test)]
    fn scheduler_for_test(&mut self) -> &mut Scheduler {
        self.run.scheduler_for_test()
    }

    /// The state of one task's slot, read through the scheduler.
    #[cfg(test)]
    pub(crate) fn task_state_for_test(
        &mut self,
        task: &promptforge_types::ids::TaskId,
    ) -> Option<crate::execute::scheduler::test_hooks::TaskState> {
        self.scheduler_for_test().task_state_for_test(task)
    }

    /// Shrinks the scheduler's chain-count bound.
    #[cfg(test)]
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.scheduler_for_test().set_max_chains_for_test(limit);
    }

    /// The number of leaf effects the run has issued so far.
    #[cfg(test)]
    pub(crate) fn leaf_requests_issued(&mut self) -> u64 {
        self.scheduler_for_test().leaf_requests_issued()
    }

    /// The run itself, for a test that answers an effect by hand.
    #[cfg(test)]
    pub(crate) fn run_for_test(&mut self) -> &mut Run {
        &mut self.run
    }

    /// The driver's cancel flag, for a test that cancels from another
    /// task.
    #[cfg(test)]
    pub(crate) fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }
}

/// Deterministically permutes `batch` under `seed`, advancing the seed
/// so successive waves differ. xorshift64: dependency-free, and one
/// seed always yields one order.
#[cfg(test)]
pub(super) fn shuffle_batch(
    mut batch: Vec<(EffectId, EffectAnswer)>,
    seed: &mut u64,
) -> Vec<(EffectId, EffectAnswer)> {
    for index in (1..batch.len()).rev() {
        *seed ^= seed.wrapping_shl(13);
        *seed ^= *seed >> 7;
        *seed ^= seed.wrapping_shl(17);
        // The modulo binds the pick to the batch; the truncated-fallback
        // arm never runs on a 64-bit target, which this suite requires.
        batch.swap(index, usize::try_from(*seed).unwrap_or(0) % (index + 1));
    }
    batch
}
