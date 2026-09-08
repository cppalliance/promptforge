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

    fn reserve(&self, capacity: usize) -> Result<RetainedPcmOwner, AudioError> {
        self.state
            .retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(capacity)
                    .filter(|total| *total <= self.state.limit)
            })
            .map_err(|_| AudioError::BufferTooLong {
                maximum_seconds: MAX_RETAINED_SECONDS,
            })?;
        Ok(RetainedPcmOwner {
            budget: self.clone(),
            capacity,
        })
    }

    fn reserve_remaining(&self) -> RetainedPcmOwner {
        let mut retained = self.state.retained.load(Ordering::Acquire);
        loop {
            let remaining = self.state.limit.saturating_sub(retained);
            match self.state.retained.compare_exchange_weak(
                retained,
                self.state.limit,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return RetainedPcmOwner {
                        budget: self.clone(),
                        capacity: remaining,
                    };
                }
                Err(actual) => retained = actual,
            }
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
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
    capacity: usize,
}

impl RetainedPcmOwner {
    fn absorb(&mut self, mut other: Self) {
        debug_assert!(Arc::ptr_eq(&self.budget.state, &other.budget.state));
        self.capacity += other.capacity;
        other.capacity = 0;
    }

    fn take_all(&mut self) -> Self {
        let capacity = std::mem::take(&mut self.capacity);
        Self {
            budget: self.budget.clone(),
            capacity,
        }
    }

    fn release(&mut self, capacity: usize) {
        debug_assert!(capacity <= self.capacity);
        self.capacity -= capacity;
        self.budget
            .state
            .retained
            .fetch_sub(capacity, Ordering::AcqRel);
    }

    #[cfg(test)]
    fn samples(&self) -> usize {
        self.capacity
    }
}

impl Drop for RetainedPcmOwner {
    fn drop(&mut self) {
        self.budget
            .state
            .retained
            .fetch_sub(self.capacity, Ordering::AcqRel);
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

    pub(super) fn retain_tail(
        mut samples: Vec<f32>,
        owner: RetainedPcmOwner,
        retained_samples: usize,
    ) -> Self {
        debug_assert!(retained_samples <= samples.len());
        let released = samples.len() - retained_samples;
        samples.drain(..released);
        Self { samples, owner }
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
        let owner = RetainedPcmOwner {
            budget,
            capacity: 0,
        };
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

        if samples.is_empty() {
            return Ok(());
        }
        let incoming = self.owner.budget.reserve(samples.capacity())?;
        if self.samples.is_empty() && self.samples.capacity() == 0 {
            self.samples = samples;
            self.owner.absorb(incoming);
            return Ok(());
        }

        let old_capacity = self.samples.capacity();
        if next_len > old_capacity {
            let mut headroom = self.owner.budget.reserve_remaining();
            let required = next_len - old_capacity;
            if required > headroom.capacity || self.samples.try_reserve_exact(required).is_err() {
                return Err(AudioError::BufferTooLong {
                    maximum_seconds: MAX_RETAINED_SECONDS,
                });
            }
            if self.samples.capacity() - old_capacity > headroom.capacity {
                self.samples.shrink_to(next_len);
            }
            let allocated = self.samples.capacity() - old_capacity;
            if allocated > headroom.capacity {
                return Err(AudioError::BufferTooLong {
                    maximum_seconds: MAX_RETAINED_SECONDS,
                });
            }
            headroom.release(headroom.capacity - allocated);
            self.owner.absorb(headroom);
        }
        self.samples.extend(samples);
        drop(incoming);
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
    pub(super) fn budget_probe(&self) -> PcmBudgetProbe {
        PcmBudgetProbe {
            state: Arc::clone(&self.owner.budget.state),
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(super) fn retained_samples(&self) -> usize {
        self.owner.budget.retained_samples()
    }

    pub(super) fn copy_range(&self, range: Range<u64>) -> Result<RetainedPcm, AudioError> {
        let local = self.local_range(&range);
        let mut owner = self.owner.budget.reserve_remaining();
        if local.len() > owner.capacity {
            return Err(AudioError::BufferTooLong {
                maximum_seconds: MAX_RETAINED_SECONDS,
            });
        }
        let mut samples = Vec::new();
        if samples.try_reserve_exact(local.len()).is_err() || samples.capacity() > owner.capacity {
            return Err(AudioError::BufferTooLong {
                maximum_seconds: MAX_RETAINED_SECONDS,
            });
        }
        owner.release(owner.capacity - samples.capacity());
        samples.extend_from_slice(&self.samples[local]);
        Ok(RetainedPcm { samples, owner })
    }

    pub(super) fn transfer_range(
        &mut self,
        range: Range<u64>,
    ) -> Result<RetainedPcm, PcmRangeError> {
        let local = self.try_local_range(&range)?;
        let tail = if local.end == self.samples.len() {
            None
        } else {
            let tail_capacity = self.samples.len() - local.end;
            let mut tail_owner = self.owner.budget.reserve_remaining();
            if tail_capacity > tail_owner.capacity {
                return Err(PcmRangeError);
            }
            let mut tail = Vec::new();
            tail.try_reserve_exact(tail_capacity)
                .map_err(|_| PcmRangeError)?;
            if tail.capacity() > tail_owner.capacity {
                return Err(PcmRangeError);
            }
            tail_owner.release(tail_owner.capacity - tail.capacity());
            tail.extend_from_slice(&self.samples[local.end..]);
            self.samples.truncate(local.end);
            Some((tail, tail_owner))
        };
        self.samples.drain(..local.start);
        let samples = std::mem::take(&mut self.samples);
        let transferred = self.owner.take_all();
        if let Some((tail, tail_owner)) = tail {
            self.samples = tail;
            self.owner.absorb(tail_owner);
        }
        self.origin = range.end;
        Ok(RetainedPcm {
            samples,
            owner: transferred,
        })
    }

    pub(super) fn compact_to(&mut self, end: u64) -> Result<(), PcmRangeError> {
        let local = self.try_local_range(&(self.origin..end))?;
        self.samples.drain(..local.end);
        self.origin = end;
        if self.samples.is_empty() {
            self.samples = Vec::new();
            let released = self.owner.capacity;
            self.owner.release(released);
        }
        Ok(())
    }

    pub(super) fn restore_prefix(
        &mut self,
        range: Range<u64>,
        mut prefix: RetainedPcm,
    ) -> Result<(), PcmRangeError> {
        let length = range
            .end
            .checked_sub(range.start)
            .and_then(|length| usize::try_from(length).ok())
            .ok_or(PcmRangeError)?;
        if range.end != self.origin || length != prefix.len() {
            return Err(PcmRangeError);
        }
        if prefix.samples.capacity() - prefix.samples.len() < self.samples.len() {
            return Err(PcmRangeError);
        }
        prefix.samples.append(&mut self.samples);
        let previous_owner = std::mem::replace(&mut self.owner, prefix.owner);
        self.samples = prefix.samples;
        self.origin = range.start;
        drop(previous_owner);
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
mod tests;
