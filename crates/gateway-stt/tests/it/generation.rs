//! Generation replacement and ownership integration tests.

#![expect(
    clippy::expect_used,
    reason = "integration tests panic with the failed ownership invariant"
)]

use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    ScriptedDecoder, ScriptedModelFactory, begin_scripted_replacement, generation_counts,
    generation_ownership, scripted_service,
};

use crate::common::transcribe_batch;

const WAIT: Duration = Duration::from_secs(2);

fn factory(decoder: &ScriptedDecoder) -> ScriptedModelFactory {
    ScriptedModelFactory::new(decoder.clone())
}

fn service(decoder: &ScriptedDecoder) -> SpeechService {
    scripted_service(factory(decoder), 15, 500).expect("scripted generation starts")
}

#[test]
fn request_and_worker_job_ownership_are_counted_independently() {
    let decoder = ScriptedDecoder::new();
    let service = service(&decoder);
    let request = generation_ownership(&service).expect("open generation admits a request");
    assert_eq!(generation_counts(&service), Some((1, 0)));

    let job = request
        .own_worker_job()
        .expect("the admitted request owns a worker job");
    assert_eq!(generation_counts(&service), Some((1, 1)));

    drop(request);
    assert_eq!(
        generation_counts(&service),
        Some((0, 1)),
        "request cancellation cannot report false worker idleness"
    );
    drop(job);
    assert_eq!(generation_counts(&service), Some((0, 0)));
    service.shutdown();
}

#[test]
fn a_quiescence_deadline_reopens_the_same_snapshot_with_a_fresh_epoch() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let stale = generation_ownership(&service).expect("old generation admits");
    let stale_epoch = stale.epoch();
    let replacement = ScriptedDecoder::new();

    let error = begin_scripted_replacement(
        &service,
        factory(&replacement),
        false,
        Duration::from_millis(20),
    )
    .expect_err("owned old request prevents bounded quiescence");

    assert!(error.to_string().contains("quiescence deadline"));
    assert!(stale.is_replaced(), "closing cancels the old session epoch");
    assert!(
        replacement.creation_thread().is_none(),
        "a failed drain never loads replacement model memory"
    );
    let fresh = generation_ownership(&service).expect("deadline reopens admission");
    assert_ne!(fresh.epoch(), stale_epoch);
    assert!(!fresh.is_replaced());
    assert!(service.status().ready());

    drop((fresh, stale));
    service.shutdown();
}

#[test]
fn an_unrepresentable_deadline_leaves_the_same_snapshot_open() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let admitted = generation_ownership(&service).expect("old generation admits");
    let epoch = admitted.epoch();
    let replacement = ScriptedDecoder::new();

    let error = begin_scripted_replacement(&service, factory(&replacement), false, Duration::MAX)
        .expect_err("an unrepresentable deadline rejects replacement");

    assert!(error.to_string().contains("quiescence deadline"));
    assert!(
        !admitted.is_replaced(),
        "deadline validation happens before admission or epoch mutation"
    );
    assert!(
        replacement.creation_thread().is_none(),
        "invalid control-plane input never loads replacement model memory"
    );
    let fresh = generation_ownership(&service).expect("the original snapshot remains open");
    assert_eq!(fresh.epoch(), epoch);
    assert!(!fresh.is_replaced());
    assert!(service.status().ready());

    drop((fresh, admitted));
    service.shutdown();
}

#[test]
fn replacement_is_serial_and_publishes_one_complete_snapshot() {
    let service = SpeechService::new();
    let first_decoder = ScriptedDecoder::new();
    let first = begin_scripted_replacement(&service, factory(&first_decoder), false, WAIT)
        .expect("first stages");
    let second_interim = ScriptedDecoder::new();
    let second_final = ScriptedDecoder::new();
    let second_factory = factory(&second_interim)
        .with_final(second_final.clone())
        .with_gpu_available(true);
    let contender_service = service.clone();
    let (finished_tx, finished_rx) = mpsc::channel();
    let contender = std::thread::spawn(move || {
        drop(finished_tx.send(begin_scripted_replacement(
            &contender_service,
            second_factory,
            true,
            WAIT,
        )));
    });

    assert!(
        matches!(
            finished_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a second replacement waits for ownership of the first transaction"
    );
    assert!(second_interim.creation_thread().is_none());

    service
        .abort_replacement(first)
        .expect("aborting the first replacement leaves no old generation");
    let second = finished_rx
        .recv_timeout(WAIT)
        .expect("second replacement resumes")
        .expect("second replacement stages");
    contender.join().expect("replacement contender joins");
    service
        .commit_replacement(second)
        .expect("complete generation publishes");

    let status = service.status();
    assert!(status.ready());
    assert!(status.gpu());
    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim", "scripted-final", "realtime-transcribe"]
    );
    assert!(first_decoder.worker_dropped());
    service.shutdown();
    assert!(second_interim.worker_dropped());
    assert!(second_final.worker_dropped());
}

#[tokio::test]
async fn active_replacement_drains_request_and_job_before_unload_and_publication() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let next_interim = ScriptedDecoder::new();
    let next_final = ScriptedDecoder::new();
    let next_factory = factory(&next_interim)
        .with_final(next_final.clone())
        .with_gpu_available(true);
    let replacement = old
        .with_next_decode_blocked(
            WAIT,
            || {
                let request_service = service.clone();
                async move {
                    (tokio::spawn(async move {
                        transcribe_batch(request_service, "scripted-interim", &[0.25; 16]).await
                    }),)
                }
            },
            |(request,)| async {
                assert_eq!(generation_counts(&service), Some((1, 1)));
                let replacement_service = service.clone();
                let replacement = tokio::task::spawn_blocking(move || {
                    begin_scripted_replacement(&replacement_service, next_factory, true, WAIT)
                });

                tokio::time::timeout(WAIT, async {
                    while generation_counts(&service) != Some((0, 1)) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("request ownership drains while the parked job remains");
                tokio::time::timeout(WAIT, request)
                    .await
                    .expect("canceled old request returns")
                    .expect("old request task joins");
                let draining = service.status();
                assert!(
                    draining.configured(),
                    "draining keeps the published configuration"
                );
                assert!(!draining.ready(), "closed admission is not ready");
                assert_eq!(
                    draining.generation(),
                    None,
                    "draining never exposes a generation that refuses admission"
                );
                assert!(
                    service.models().is_empty(),
                    "draining publishes no discoverable speech model"
                );
                assert!(
                    next_interim.creation_thread().is_none()
                        && next_final.creation_thread().is_none(),
                    "replacement construction waits for every old worker job"
                );
                assert!(
                    !old.worker_dropped(),
                    "the running old worker remains owned until native work returns"
                );
                (replacement,)
            },
        )
        .await
        .expect("the old generation owns one blocked native-equivalent job");
    let (replacement,) = replacement;
    let replacement = tokio::time::timeout(WAIT, replacement)
        .await
        .expect("active replacement finishes after old work drains")
        .expect("replacement task joins")
        .expect("replacement stages");
    assert!(
        old.worker_dropped(),
        "old workers unload before the staged replacement returns"
    );
    assert!(next_interim.creation_thread().is_some());
    assert!(next_final.creation_thread().is_some());

    service
        .commit_replacement(replacement)
        .expect("the complete replacement publishes");
    assert_eq!(generation_counts(&service), Some((0, 0)));
    assert!(service.status().ready());
    assert!(service.status().gpu());
    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim", "scripted-final", "realtime-transcribe"]
    );

    service.shutdown();
    assert!(next_interim.worker_dropped());
    assert!(next_final.worker_dropped());
}

#[test]
fn aborting_a_staged_replacement_reconstructs_the_old_generation() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let old_generation = service.status().generation();
    let next = ScriptedDecoder::new();
    let replacement = begin_scripted_replacement(&service, factory(&next), false, WAIT)
        .expect("replacement stages");

    assert!(!service.status().ready(), "staged state stays unpublished");
    service
        .abort_replacement(replacement)
        .expect("determinate abort reconstructs the old specification");

    let restored = service.status();
    assert!(restored.ready());
    assert_ne!(
        restored.generation(),
        old_generation,
        "reconstruction publishes a fresh generation"
    );
    let request = generation_ownership(&service).expect("reconstructed generation admits");
    assert!(
        request.own_worker_job().is_some(),
        "the reconstructed generation accepts worker ownership"
    );
    drop(request);
    assert!(next.worker_dropped(), "the staged worker is joined");
    service.shutdown();
}

#[test]
fn dropping_a_staged_replacement_restores_admission_without_publishing_it() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let old_generation = service.status().generation();
    let next_interim = ScriptedDecoder::new();
    let next_final = ScriptedDecoder::new();
    let replacement = begin_scripted_replacement(
        &service,
        factory(&next_interim).with_final(next_final.clone()),
        true,
        WAIT,
    )
    .expect("replacement stages");

    assert!(!service.status().ready(), "staging closes old admission");
    assert!(
        service.models().is_empty(),
        "an uncommitted replacement publishes no models"
    );
    drop(replacement);

    let restored = service.status();
    assert!(restored.ready(), "drop reconstructs the old generation");
    assert_ne!(
        restored.generation(),
        old_generation,
        "reconstruction publishes a fresh generation"
    );
    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim"],
        "drop restores the old model set instead of publishing the staged target"
    );
    assert_eq!(
        generation_counts(&service),
        Some((0, 0)),
        "replacement drop leaks no admission or worker ownership"
    );
    let request = generation_ownership(&service).expect("restored generation admits");
    let job = request
        .own_worker_job()
        .expect("restored admission owns worker work");
    assert_eq!(generation_counts(&service), Some((1, 1)));
    drop((job, request));
    assert_eq!(generation_counts(&service), Some((0, 0)));
    assert!(next_interim.worker_dropped());
    assert!(next_final.worker_dropped());
    service.shutdown();
}

#[test]
fn determinate_start_failure_reconstructs_the_old_generation() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let failed = ScriptedDecoder::new();

    let error = begin_scripted_replacement(
        &service,
        factory(&failed).with_interim_failure("determinate startup failure"),
        false,
        WAIT,
    )
    .expect_err("replacement construction fails");

    assert!(
        format!("{error:?}").contains("determinate startup failure"),
        "the original determinate failure remains visible: {error:?}"
    );
    assert!(
        service.status().ready(),
        "a determinate staged failure reconstructs old speech"
    );
    assert!(
        generation_ownership(&service)
            .and_then(|request| request.own_worker_job())
            .is_some(),
        "the old specification starts an admitting worker before failure returns"
    );
    service.shutdown();
}

#[test]
fn determinate_start_failure_reports_failed_old_generation_reconstruction() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    old.fail_next_construction("rollback reconstruction sentinel");
    let failed = ScriptedDecoder::new();

    let error = begin_scripted_replacement(
        &service,
        factory(&failed).with_interim_failure("determinate startup sentinel"),
        false,
        WAIT,
    )
    .expect_err("both replacement and reconstruction fail");
    let debug = format!("{error:?}");

    assert!(debug.contains("determinate startup sentinel"), "{debug}");
    assert!(
        debug.contains("rollback reconstruction sentinel"),
        "{debug}"
    );
    assert!(
        !service.status().ready(),
        "failed reconstruction cannot claim speech remains available"
    );
}

#[test]
fn rollback_attempts_reconstruction_after_staged_worker_shutdown_fails() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let next = ScriptedDecoder::new();
    let replacement = begin_scripted_replacement(&service, factory(&next), false, WAIT)
        .expect("replacement stages");
    next.panic_on_drop();
    old.fail_next_construction("reconstruction after cleanup sentinel");

    let error = service
        .abort_replacement(replacement)
        .expect_err("cleanup and reconstruction failures are aggregated");
    let debug = format!("{error:?}");

    assert!(debug.contains("ShutdownPanicked"), "{debug}");
    assert!(
        debug.contains("reconstruction after cleanup sentinel"),
        "{debug}"
    );
    assert!(!service.status().ready());
}

#[tokio::test]
async fn canceled_request_keeps_its_worker_job_owned_until_decode_returns() {
    let old = ScriptedDecoder::new();
    let service = service(&old);
    let replacement = ScriptedDecoder::new();
    old.with_next_decode_blocked(
        WAIT,
        || {
            let request_service = service.clone();
            async move {
                (tokio::spawn(async move {
                    transcribe_batch(request_service, "scripted-interim", &[0.25; 16]).await
                }),)
            }
        },
        |(request,)| async {
            assert_eq!(generation_counts(&service), Some((1, 1)));
            request.abort();
            assert!(
                request
                    .await
                    .expect_err("request is canceled")
                    .is_cancelled()
            );
            tokio::time::timeout(WAIT, async {
                while generation_counts(&service) != Some((0, 1)) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("request ownership drops while worker ownership remains");
            assert_eq!(generation_counts(&service), Some((0, 1)));

            let replacement_control = replacement.clone();
            let replacement_service = service.clone();
            let attempt = tokio::task::spawn_blocking(move || {
                begin_scripted_replacement(
                    &replacement_service,
                    factory(&replacement_control),
                    false,
                    Duration::from_millis(20),
                )
            })
            .await
            .expect("replacement attempt joins");
            let error = attempt.expect_err("the live worker job prevents quiescence");
            assert!(error.to_string().contains("quiescence deadline"));
            assert!(replacement.creation_thread().is_none());
        },
    )
    .await
    .expect("decode reaches the blocked native-equivalent scenario");
    tokio::time::timeout(WAIT, async {
        while generation_counts(&service) != Some((0, 0)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker ownership drains after native decode returns");
    service.shutdown();
}

#[test]
fn shutdown_wins_a_race_with_staged_publication() {
    for _ in 0..8 {
        let service = SpeechService::new();
        let decoder = ScriptedDecoder::new();
        let replacement = begin_scripted_replacement(&service, factory(&decoder), false, WAIT)
            .expect("generation stages");
        let barrier = Arc::new(Barrier::new(2));
        let commit_barrier = Arc::clone(&barrier);
        let commit_service = service.clone();
        let commit = std::thread::spawn(move || {
            commit_barrier.wait();
            commit_service.commit_replacement(replacement)
        });

        barrier.wait();
        service.shutdown();
        let outcome = commit.join().expect("commit contender joins");
        if let Err(error) = outcome {
            assert!(error.to_string().contains("invalidated"));
        }
        assert!(
            !service.status().ready(),
            "shutdown never permits a stale staged generation to survive"
        );
        assert!(decoder.worker_dropped());
    }
}
