//! Deterministic decoder fixtures for downstream integration tests.

/// Native asset resolution for ignored integration tests.
pub mod native;

use std::sync::mpsc::{TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use crate::{DecodeMode, Decoder, ModelFactory, TranscribeError};

mod scenarios;
pub use scenarios::ScriptedDecoder;

struct ConstructionBlock(ScriptedDecoder);

impl Drop for ConstructionBlock {
    fn drop(&mut self) {
        self.0.release_construction();
    }
}

/// A role-specific scripted [`ModelFactory`] for test engines.
#[derive(Debug)]
pub struct ScriptedModelFactory {
    interim: ScriptedDecoder,
    final_decoder: Option<ScriptedDecoder>,
    interim_failure: Option<String>,
    final_failure: Option<String>,
    panic_interim: bool,
    panic_final: bool,
    gpu_available: bool,
}

impl ScriptedModelFactory {
    /// Creates an interim-only scripted factory.
    #[must_use]
    pub fn new(interim: ScriptedDecoder) -> Self {
        Self {
            interim,
            final_decoder: None,
            interim_failure: None,
            final_failure: None,
            panic_interim: false,
            panic_final: false,
            gpu_available: false,
        }
    }

    /// Installs the optional final-role decoder.
    #[must_use]
    pub fn with_final(mut self, decoder: ScriptedDecoder) -> Self {
        self.final_decoder = Some(decoder);
        self
    }

    /// Makes interim construction fail with the supplied message.
    #[must_use]
    pub fn with_interim_failure(mut self, message: impl Into<String>) -> Self {
        self.interim_failure = Some(message.into());
        self
    }

    /// Makes final construction fail with the supplied message.
    #[must_use]
    pub fn with_final_failure(mut self, message: impl Into<String>) -> Self {
        self.final_failure = Some(message.into());
        self
    }

    /// Makes interim construction panic.
    #[must_use]
    pub fn with_interim_panic(mut self) -> Self {
        self.panic_interim = true;
        self
    }

    /// Makes final construction panic.
    #[must_use]
    pub fn with_final_panic(mut self) -> Self {
        self.panic_final = true;
        self
    }

    /// Sets the hardware-acceleration fact reported by the fixture.
    #[must_use]
    pub fn with_gpu_available(mut self, available: bool) -> Self {
        self.gpu_available = available;
        self
    }

    /// Returns the fixture hardware-acceleration fact.
    #[must_use]
    pub fn gpu_available(&self) -> bool {
        self.gpu_available
    }

    /// Runs a bounded scenario while all configured decoders are constructing.
    ///
    /// Construction starts on a scoped thread. The scenario runs only after
    /// every role is parked and before a result is available. The result must
    /// then arrive within `result_timeout`; every return or unwind releases all
    /// parked roles before joining the construction thread.
    ///
    /// # Panics
    /// Panics when construction or the scenario panics, or when construction
    /// completes before every configured role is observed parked.
    pub fn with_construction_blocked<Start, Result, Scenario, Observation>(
        self,
        rendezvous_timeout: Duration,
        result_timeout: Duration,
        start: Start,
        while_blocked: Scenario,
    ) -> Option<(Result, Observation)>
    where
        Start: FnOnce(Self) -> Result + Send,
        Result: Send,
        Scenario: FnOnce() -> Observation,
    {
        let decoders = self.construction_decoders();
        for decoder in &decoders {
            decoder.arm_construction();
        }

        std::thread::scope(|scope| {
            let blocks = decoders
                .iter()
                .cloned()
                .map(ConstructionBlock)
                .collect::<Vec<_>>();
            let (result_tx, result_rx) = sync_channel(1);
            let constructor = scope.spawn(move || {
                drop(result_tx.send(start(self)));
            });

            if !wait_until_all_constructing(&decoders, rendezvous_timeout) {
                drop(blocks);
                constructor
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                return None;
            }
            assert!(
                matches!(result_rx.try_recv(), Err(TryRecvError::Empty)),
                "construction completed before every configured role was observed parked"
            );

            let observation = while_blocked();
            let result = result_rx.recv_timeout(result_timeout).ok();
            drop(blocks);
            constructor
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            result.map(|result| (result, observation))
        })
    }

    fn construction_decoders(&self) -> Vec<ScriptedDecoder> {
        let mut decoders = vec![self.interim.clone()];
        decoders.extend(self.final_decoder.iter().cloned());
        decoders
    }
}

impl ModelFactory for ScriptedModelFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        let (decoder, failure, panic) = match mode {
            DecodeMode::Interim => (
                Some(&self.interim),
                &self.interim_failure,
                self.panic_interim,
            ),
            DecodeMode::Final => (
                self.final_decoder.as_ref(),
                &self.final_failure,
                self.panic_final,
            ),
        };
        assert!(!panic, "scripted {mode:?} factory panic");
        if let Some(message) = failure {
            return Err(TranscribeError::InvalidConfig(message.clone()));
        }
        let Some(decoder) = decoder else {
            return Ok(None);
        };
        if let Some(message) = decoder.take_construction_error() {
            return Err(TranscribeError::InvalidConfig(message));
        }
        decoder.mark_created();
        Ok(Some(decoder.worker()))
    }
}

fn wait_until_all_constructing(decoders: &[ScriptedDecoder], timeout: Duration) -> bool {
    let started = Instant::now();
    decoders.iter().all(|decoder| {
        decoder.wait_until_construction_parked(timeout.saturating_sub(started.elapsed()))
    })
}

#[cfg(test)]
mod tests;
