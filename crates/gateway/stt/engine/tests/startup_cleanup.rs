//! Partial-startup and role-ordered worker cleanup regressions.

#![cfg(feature = "test-fixtures")]

use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
use gateway_stt_engine::{
    DecodeMode, Decoder, EnginePolicy, ModelFactory, SttEngine, TranscribeError,
};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use thiserror as _;
use tokio as _;

fn policy() -> EnginePolicy {
    let Ok(policy) = EnginePolicy::new(15, 500, false) else {
        panic!("test policy must be valid");
    };
    policy
}

const INTERIM_SENTINEL: &str = "simultaneous interim startup failure";
const FINAL_SENTINEL: &str = "simultaneous final startup failure";

#[derive(Debug)]
struct ConcurrentStartupFailureFactory {
    rendezvous: Arc<Barrier>,
    interim_is_missing: bool,
}

impl ModelFactory for ConcurrentStartupFailureFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        self.rendezvous.wait();
        match (mode, self.interim_is_missing) {
            (DecodeMode::Interim, true) => Ok(None),
            (DecodeMode::Interim, false) => Err(TranscribeError::load_model(
                PathBuf::from(INTERIM_SENTINEL),
                std::io::Error::other(INTERIM_SENTINEL),
            )),
            (DecodeMode::Final, _) => Err(TranscribeError::load_model(
                PathBuf::from(FINAL_SENTINEL),
                std::io::Error::other(FINAL_SENTINEL),
            )),
            _ => unreachable!("the startup cleanup factory scripts only interim and final modes"),
        }
    }
}

fn concurrent_startup_failure(interim_is_missing: bool) -> TranscribeError {
    let Err(error) = SttEngine::new(
        ConcurrentStartupFailureFactory {
            rendezvous: Arc::new(Barrier::new(2)),
            interim_is_missing,
        },
        policy(),
    ) else {
        panic!("both role outcomes prevent construction");
    };
    error
}

#[test]
fn simultaneous_role_failures_preserve_both_exact_outcomes() {
    let TranscribeError::StartupFailures { failures, .. } = concurrent_startup_failure(false)
    else {
        panic!("simultaneous role failures must be aggregated");
    };
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0].to_string(),
        format!("load transcription model {INTERIM_SENTINEL}")
    );
    assert_eq!(
        failures[1].to_string(),
        format!("load transcription model {FINAL_SENTINEL}")
    );
}

#[test]
fn missing_interim_preserves_the_simultaneous_final_failure() {
    let TranscribeError::StartupFailures { failures, .. } = concurrent_startup_failure(true) else {
        panic!("the missing interim and final failure must be aggregated");
    };
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0].to_string(),
        "invalid STT configuration: the interim decoder is required"
    );
    assert_eq!(
        failures[1].to_string(),
        format!("load transcription model {FINAL_SENTINEL}")
    );
}

#[test]
fn oversized_public_startup_timeout_returns_exact_invalid_configuration() {
    let interim = ScriptedDecoder::new();
    let error = SttEngine::new(
        ScriptedModelFactory::new(interim.clone()),
        policy().with_startup_timeout(Duration::MAX),
    )
    .expect_err("an unrepresentable absolute deadline must be rejected");
    assert_eq!(
        error.to_string(),
        "invalid STT configuration: stt.startup_timeout is too large"
    );
    assert_eq!(
        interim.creation_thread(),
        None,
        "deadline validation precedes worker construction"
    );
}

#[test]
fn final_first_startup_failure_preserves_interim_cleanup_panic() {
    const SENTINEL: &str = "scripted final startup with cleanup sentinel";

    let interim = ScriptedDecoder::new();
    interim.panic_on_drop();
    let Err(error) = SttEngine::new(
        ScriptedModelFactory::new(interim.clone()).with_final_failure(SENTINEL),
        policy(),
    ) else {
        panic!("final startup and interim cleanup must both fail");
    };
    let TranscribeError::StartupCleanup {
        startup, cleanup, ..
    } = error
    else {
        panic!("startup and cleanup failures must both be typed");
    };
    assert_eq!(
        startup.to_string(),
        format!("invalid STT configuration: {SENTINEL}")
    );
    assert_eq!(cleanup.len(), 1);
    assert_eq!(
        cleanup[0].to_string(),
        "transcription worker panicked during shutdown"
    );
    assert!(interim.worker_dropped());
}

#[test]
fn both_workers_start_concurrently_under_one_absolute_deadline() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let factory = ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone());
    let policy = policy().with_startup_timeout(Duration::from_millis(200));
    let (result, ()) = factory
        .with_construction_blocked(
            Duration::from_secs(1),
            Duration::from_millis(350),
            |factory| SttEngine::new(factory, policy),
            || {
                assert_eq!(
                    interim.creation_thread(),
                    None,
                    "interim is observed parked before construction can complete"
                );
                assert_eq!(
                    final_decoder.creation_thread(),
                    None,
                    "final is observed parked before construction can complete"
                );
            },
        )
        .expect("both roles park before the bounded constructor result arrives");
    let error = result.expect_err("both parked workers share one startup deadline");
    let TranscribeError::StartupFailures { failures, .. } = error else {
        panic!("both role-specific timeouts must be preserved");
    };
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0].to_string(),
        "interim transcription worker startup timed out"
    );
    assert_eq!(
        failures[1].to_string(),
        "final transcription worker startup timed out"
    );
}

#[test]
fn shutdown_surfaces_interim_first_panic_and_still_joins_final() {
    let interim = ScriptedDecoder::new();
    interim.panic_on_drop();
    let final_decoder = ScriptedDecoder::new();
    let Ok(engine) = SttEngine::new(
        ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone()),
        policy(),
    ) else {
        panic!("scripted workers must start");
    };

    let Err(error) = engine.shutdown() else {
        panic!("interim shutdown must fail");
    };
    assert_eq!(
        error.to_string(),
        "transcription worker panicked during shutdown"
    );
    assert!(interim.worker_dropped());
    assert!(final_decoder.worker_dropped());
}

#[test]
fn shutdown_surfaces_final_panic_after_interim_first_cleanup() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.panic_on_drop();
    let Ok(engine) = SttEngine::new(
        ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone()),
        policy(),
    ) else {
        panic!("scripted workers must start");
    };

    let Err(error) = engine.shutdown() else {
        panic!("final shutdown must fail");
    };
    assert_eq!(
        error.to_string(),
        "transcription worker panicked during shutdown"
    );
    assert!(interim.worker_dropped());
    assert!(final_decoder.worker_dropped());

    let Err(repeated) = engine.shutdown() else {
        panic!("the final shutdown panic must remain visible");
    };
    assert_eq!(
        repeated.to_string(),
        "transcription worker panicked during shutdown"
    );
}

#[test]
fn shutdown_aggregates_both_panics_and_repeats_the_complete_failure_set() {
    let interim = ScriptedDecoder::new();
    interim.panic_on_drop();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.panic_on_drop();
    let Ok(engine) = SttEngine::new(
        ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone()),
        policy(),
    ) else {
        panic!("scripted workers must start");
    };

    for call in 1..=2 {
        let Err(TranscribeError::ShutdownFailures { cleanup, .. }) = engine.shutdown() else {
            panic!("shutdown call {call} must report both worker panics");
        };
        assert_eq!(
            cleanup.len(),
            2,
            "shutdown call {call} preserves both failures"
        );
        assert!(
            cleanup
                .iter()
                .all(|error| error.to_string() == "transcription worker panicked during shutdown")
        );
    }
    assert!(interim.worker_dropped());
    assert!(final_decoder.worker_dropped());
}
