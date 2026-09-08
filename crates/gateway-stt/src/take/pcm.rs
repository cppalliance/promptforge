use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gateway_stt_engine::EnginePolicy;

use crate::audio::AudioError;

const MAX_RETAINED_SECONDS: usize = 30;
const MAX_RETAINED_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * MAX_RETAINED_SECONDS;

#[derive(Debug, Default)]
struct BudgetState {
    retained: AtomicUsize,
    limit: usize,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedPcmBudget {
    state: Arc<BudgetState>,
}

impl Default for RetainedPcmBudget {
    fn default() -> Self {
        Self::with_limit(MAX_RETAINED_SAMPLES)
    }
}

impl RetainedPcmBudget {
    pub(super) fn with_limit(limit: usize) -> Self {
        Self {
            state: Arc::new(BudgetState {
                retained: AtomicUsize::new(0),
                limit,
            }),
        }
    }

    fn reserve(&self, samples: usize) -> Result<RetainedPcmOwner, AudioError> {
        self.state
            .retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(samples)
                    .filter(|total| *total <= self.state.limit)
            })
            .map_err(|_| AudioError::BufferTooLong {
                maximum_seconds: MAX_RETAINED_SECONDS,
            })?;
        Ok(RetainedPcmOwner {
            budget: self.clone(),
            samples,
        })
    }

    #[cfg(test)]
    fn retained_samples(&self) -> usize {
        self.state.retained.load(Ordering::Acquire)
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct PcmBudgetProbe {
    state: Arc<BudgetState>,
}

#[cfg(test)]
impl PcmBudgetProbe {
    pub(crate) fn retained_samples(&self) -> usize {
        self.state.retained.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub(crate) struct RetainedPcmOwner {
    budget: RetainedPcmBudget,
    samples: usize,
}

impl RetainedPcmOwner {
    fn absorb(&mut self, mut other: Self) {
        debug_assert!(Arc::ptr_eq(&self.budget.state, &other.budget.state));
        self.samples += other.samples;
        other.samples = 0;
    }

    fn transfer(&mut self, samples: usize) -> Self {
        debug_assert!(samples <= self.samples);
        self.samples -= samples;
        Self {
            budget: self.budget.clone(),
            samples,
        }
    }

    fn release(&mut self, samples: usize) {
        debug_assert!(samples <= self.samples);
        self.samples -= samples;
        self.budget
            .state
            .retained
            .fetch_sub(samples, Ordering::AcqRel);
    }

    #[cfg(test)]
    fn samples(&self) -> usize {
        self.samples
    }
}

impl Drop for RetainedPcmOwner {
    fn drop(&mut self) {
        self.budget
            .state
            .retained
            .fetch_sub(self.samples, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(crate) struct RetainedPcm {
    samples: Vec<f32>,
    owner: RetainedPcmOwner,
}

impl RetainedPcm {
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub(crate) fn into_decode(self) -> (Vec<f32>, RetainedPcmOwner) {
        (self.samples, self.owner)
    }
}

#[derive(Debug)]
pub(super) struct RollingPcm {
    origin: u64,
    samples: Vec<f32>,
    owner: RetainedPcmOwner,
}

impl RollingPcm {
    pub(super) fn new(budget: RetainedPcmBudget) -> Self {
        let owner = RetainedPcmOwner { budget, samples: 0 };
        Self {
            origin: 0,
            samples: Vec::new(),
            owner,
        }
    }

    pub(super) fn append(&mut self, samples: Vec<f32>) -> Result<(), AudioError> {
        let next_len =
            self.samples
                .len()
                .checked_add(samples.len())
                .ok_or(AudioError::BufferTooLong {
                    maximum_seconds: MAX_RETAINED_SECONDS,
                })?;
        let _ = u64::try_from(next_len)
            .ok()
            .and_then(|length| self.origin.checked_add(length))
            .ok_or(AudioError::BufferTooLong {
                maximum_seconds: MAX_RETAINED_SECONDS,
            })?;
        let owner = self.owner.budget.reserve(samples.len())?;
        self.samples.extend(samples);
        self.owner.absorb(owner);
        Ok(())
    }

    pub(super) const fn origin(&self) -> u64 {
        self.origin
    }

    pub(super) fn end(&self) -> u64 {
        let Ok(length) = u64::try_from(self.samples.len()) else {
            unreachable!("retained PCM length always fits u64");
        };
        self.origin + length
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(super) fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) fn snapshot_from(&self, start: u64, window_samples: usize) -> Vec<f32> {
        let start = usize::try_from(start.saturating_sub(self.origin))
            .unwrap_or(usize::MAX)
            .min(self.samples.len());
        let samples = &self.samples[start..];
        samples[samples.len().saturating_sub(window_samples)..].to_vec()
    }

    #[cfg(test)]
    pub(super) fn retained_samples(&self) -> usize {
        self.owner.budget.state.retained.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn budget_probe(&self) -> PcmBudgetProbe {
        PcmBudgetProbe {
            state: Arc::clone(&self.owner.budget.state),
        }
    }

    pub(super) fn copy_range(&self, range: Range<u64>) -> Result<RetainedPcm, AudioError> {
        let local = self.local_range(&range);
        let owner = self.owner.budget.reserve(local.len())?;
        Ok(RetainedPcm {
            samples: self.samples[local].to_vec(),
            owner,
        })
    }

    pub(super) fn transfer_range(
        &mut self,
        range: Range<u64>,
    ) -> Result<RetainedPcm, PcmRangeError> {
        let local = self.try_local_range(&range)?;
        let transferred = self.owner.transfer(local.len());
        self.owner.release(local.start);
        let samples = self.samples.drain(..local.end).skip(local.start).collect();
        self.origin = range.end;
        Ok(RetainedPcm {
            samples,
            owner: transferred,
        })
    }

    pub(super) fn compact_to(&mut self, end: u64) -> Result<(), PcmRangeError> {
        let local = self.try_local_range(&(self.origin..end))?;
        self.owner.release(local.end);
        self.samples.drain(..local.end);
        self.origin = end;
        Ok(())
    }

    fn local_range(&self, range: &Range<u64>) -> Range<usize> {
        self.try_local_range(range)
            .unwrap_or_else(|_| panic!("absolute PCM range must be resident"))
    }

    fn try_local_range(&self, range: &Range<u64>) -> Result<Range<usize>, PcmRangeError> {
        if range.start < self.origin || range.start > range.end || range.end > self.end() {
            return Err(PcmRangeError);
        }
        let start = usize::try_from(range.start - self.origin).map_err(|_| PcmRangeError)?;
        let end = usize::try_from(range.end - self.origin).map_err(|_| PcmRangeError)?;
        Ok(start..end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PcmRangeError;

#[cfg(test)]
mod tests {
    use super::{RetainedPcmBudget, RollingPcm};

    #[test]
    fn miri_resident_queue_and_decode_share_one_exact_reservation() {
        let budget = RetainedPcmBudget::with_limit(12);
        let mut resident = RollingPcm::new(budget.clone());
        resident
            .append(vec![0.0; 10])
            .expect("resident PCM reserves");
        assert_eq!(budget.retained_samples(), 10);

        let queued = resident
            .transfer_range(2..6)
            .expect("the queued range transfers before compaction");
        assert_eq!(resident.origin(), 6);
        assert_eq!(resident.len(), 4);
        assert_eq!(queued.len(), 4);
        assert_eq!(budget.retained_samples(), 8);

        let (samples, decoding) = queued.into_decode();
        assert_eq!(samples.len(), 4);
        assert_eq!(decoding.samples(), 4);
        assert_eq!(budget.retained_samples(), 8);
        drop(samples);
        drop(decoding);
        assert_eq!(budget.retained_samples(), 4);
    }

    #[test]
    fn aggregate_limit_counts_resident_and_copied_interim_pcm() {
        let budget = RetainedPcmBudget::with_limit(8);
        let mut resident = RollingPcm::new(budget.clone());
        resident
            .append(vec![0.0; 6])
            .expect("resident PCM reserves");
        let interim = resident
            .copy_range(2..4)
            .expect("interim PCM reserves independently");
        assert_eq!(budget.retained_samples(), 8);
        assert!(resident.copy_range(0..1).is_err());
        drop(interim);
        assert_eq!(budget.retained_samples(), 6);
    }

    #[test]
    fn multiple_compactions_keep_absolute_origins() {
        let budget = RetainedPcmBudget::with_limit(16);
        let mut resident = RollingPcm::new(budget);
        resident
            .append((0_u8..12).map(f32::from).collect())
            .expect("resident PCM reserves");

        let first = resident
            .transfer_range(2..4)
            .expect("first range transfers");
        assert_eq!(first.samples(), &[2.0, 3.0]);
        drop(first);
        resident.append(vec![12.0, 13.0]).expect("PCM appends");
        let second = resident
            .transfer_range(8..12)
            .expect("second absolute range transfers");
        assert_eq!(second.samples(), &[8.0, 9.0, 10.0, 11.0]);
        assert_eq!(resident.origin(), 12);
        assert_eq!(resident.samples(), &[12.0, 13.0]);
    }
}
