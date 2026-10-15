//! Tests that blocked scripted construction times out and releases cleanly.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;

#[test]
fn parked_construction_has_a_bounded_classified_outcome() {
    let decoder = ScriptedDecoder::new();
    run_timed_out_construction(&decoder);
}

#[test]
fn construction_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    let factory = ScriptedModelFactory::new(decoder.clone());
    let result = factory.with_construction_blocked(
        Duration::from_millis(10),
        WAIT,
        |factory| {
            std::thread::sleep(Duration::from_millis(50));
            SttEngine::new(factory, policy())
        },
        || (),
    );
    assert!(result.is_none(), "the rendezvous must time out");
    assert!(
        decoder.wait_until_worker_dropped(WAIT),
        "the late constructor is released and its discarded engine shuts down"
    );
    run_timed_out_construction(&decoder);
}

#[test]
fn missed_rendezvous_with_a_stuck_role_returns_instead_of_hanging() {
    let decoder = ScriptedDecoder::new();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let constructing = decoder.clone();
    std::thread::spawn(move || {
        let missed = construct_with_stuck_interim(
            &constructing,
            Duration::from_millis(10),
            Duration::from_millis(50),
        )
        .is_none();
        let _ = done_tx.send(missed);
    });
    assert_eq!(
        done_rx.recv_timeout(WAIT * 10),
        Ok(true),
        "a missed rendezvous returns None instead of hanging on the stuck role"
    );
    assert!(decoder.wait_until_worker_dropped(WAIT));
    run_timed_out_construction(&decoder);
}

#[test]
fn construction_result_timeout_releases_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    let factory = ScriptedModelFactory::new(decoder.clone());
    let result = factory.with_construction_blocked(
        WAIT,
        Duration::from_millis(10),
        |factory| SttEngine::new(factory, policy()),
        || (),
    );
    assert!(result.is_none(), "the bounded result wait must time out");
    assert!(
        decoder.wait_until_worker_dropped(WAIT),
        "releasing construction lets the discarded engine shut down"
    );
    run_timed_out_construction(&decoder);
}

#[test]
fn panicked_construction_scenario_releases_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    let factory = ScriptedModelFactory::new(decoder.clone());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        factory.with_construction_blocked(
            WAIT,
            WAIT,
            |factory| SttEngine::new(factory, policy()),
            || panic!("construction scenario panic sentinel"),
        )
    }));
    assert!(panic.is_err(), "the scenario panic must propagate");
    assert!(
        decoder.wait_until_worker_dropped(WAIT),
        "unwinding releases construction and shuts down the discarded engine"
    );
    run_timed_out_construction(&decoder);
}

#[test]
fn parked_final_construction_cleans_up_the_initialized_interim_worker() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.arm_construction();
    let factory = ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone());
    let (expire, expired) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let constructor = std::thread::spawn(move || {
        drop(result_tx.send(SttEngine::new_with_stuck_role(
            factory,
            policy(),
            DecodeMode::Final,
            &expired,
        )));
    });
    assert!(
        final_decoder.wait_until_construction_parked(WAIT),
        "final construction reaches its deterministic park"
    );
    let _ = expire.send(());
    let error = result_rx
        .recv_timeout(WAIT)
        .expect("startup returns by its deadline")
        .expect_err("parked final construction times out");
    assert!(matches!(error, TranscribeError::FinalStartupTimedOut));
    assert!(
        interim.worker_dropped(),
        "the worker initialized first is joined and cleaned up"
    );
    constructor.join().expect("constructor does not panic");
    final_decoder.release_construction();
    assert!(
        final_decoder.wait_until_worker_dropped(WAIT),
        "the abandoned constructor releases its decoder after returning"
    );
}
