use std::time::Duration;

use base64::Engine as _;
use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

use crate::audio::AudioError;
use crate::realtime::input::{InputSnapshot, UncommittedInput};
use crate::realtime::registry::SessionRegistry;
use crate::realtime::session::{Session, SessionError};
use crate::test_fixtures::{begin_scripted_replacement, scripted_service};

const WAIT: Duration = Duration::from_secs(1);

fn encoded(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[allow(
    clippy::expect_used,
    reason = "the isolated fixture constructs one session and one scripted generation"
)]
fn session_with_audio(service: &crate::SpeechService, payload: &str, budget: usize) -> Session {
    let registration = SessionRegistry::default()
        .register()
        .expect("session registers");
    let engine = service
        .state
        .active()
        .expect("scripted generation is active");
    let mut session = Session::new(registration, Some(engine.clone()));
    session.input = Some(
        UncommittedInput::first_append_with_pcm_limit(
            "item_pcm_budget".to_owned(),
            InputSnapshot::new(String::new(), true),
            Some(engine),
            payload,
            budget,
        )
        .expect("test input reserves its resident PCM"),
    );
    session
}

#[allow(
    clippy::expect_used,
    reason = "the replacement outcome is the deterministic cancellation assertion"
)]
async fn cancel_generation_epoch(service: &crate::SpeechService, session: &Session) {
    let epoch = session
        .engine
        .as_ref()
        .expect("session owns its generation")
        .epoch()
        .clone();
    let replacement_service = service.clone();
    let attempt = tokio::task::spawn_blocking(move || {
        begin_scripted_replacement(
            &replacement_service,
            ScriptedModelFactory::new(ScriptedDecoder::new()),
            false,
            Duration::from_millis(50),
        )
    })
    .await
    .expect("replacement attempt joins");
    assert!(
        attempt
            .expect_err("the live session prevents replacement")
            .to_string()
            .contains("quiescence deadline")
    );
    assert!(
        epoch.is_cancelled(),
        "replacement cancels the request epoch"
    );
}

#[allow(
    clippy::expect_used,
    reason = "the bounded worker observations are deterministic fixture assertions"
)]
async fn wait_for_decode_retirement(decoder: ScriptedDecoder, service: &crate::SpeechService) {
    assert!(
        tokio::task::spawn_blocking(move || decoder.wait_for_completed(1, WAIT))
            .await
            .expect("decode retirement observation joins"),
        "the blocked worker retires its request"
    );
    tokio::time::timeout(WAIT, async {
        while service.state.counts() != Some((1, 0)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached generation job retires");
}

#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "the scripted interim sequence must reach every asserted ownership boundary"
)]
async fn blocked_interim_keeps_exact_budget_until_worker_retirement_and_commit_retries() {
    let interim = ScriptedDecoder::new();
    let service = scripted_service(ScriptedModelFactory::new(interim.clone()), 15, 500)
        .expect("scripted generation starts");
    let payload = encoded(&vec![16_384; 12_002]);
    let mut session = session_with_audio(&service, &payload, 16_002);
    let probe = session
        .input
        .as_ref()
        .expect("input exists")
        .take()
        .pcm_budget_probe();

    interim
        .with_next_decode_blocked(
            WAIT,
            || async {
                session.schedule_interim().expect("interim schedules");
                &mut session
            },
            |session| async {
                assert_eq!(probe.retained_samples(), 16_002);
                cancel_generation_epoch(&service, session).await;
                assert!(matches!(
                    session.finish_interim().await,
                    Err(SessionError::Inference)
                ));
                assert_eq!(
                    probe.retained_samples(),
                    16_002,
                    "epoch cancellation cannot release worker-owned PCM"
                );

                assert_eq!(
                    session.commit(),
                    Err(SessionError::Audio(AudioError::BufferTooLong {
                        maximum_seconds: 30,
                    }))
                );
                assert_eq!(session.results.reserved_items(), 0);
                assert_eq!(session.committed.len(), 0);
                assert_eq!(
                    session
                        .input
                        .as_ref()
                        .expect("failed seal restores the same input")
                        .item_id(),
                    "item_pcm_budget"
                );
            },
        )
        .await
        .expect("interim reaches the blocked worker");

    wait_for_decode_retirement(interim, &service).await;
    assert_eq!(probe.retained_samples(), 8_001);
    let receipt = session.commit().expect("the exact same input retries");
    assert_eq!(receipt.item_id(), "item_pcm_budget");
    assert_eq!(session.results.reserved_items(), 1);
    assert_eq!(session.committed.len(), 1);
    drop(session);
    service.shutdown();
}

#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "the scripted final sequence must reach every asserted ownership boundary"
)]
async fn blocked_final_keeps_budget_after_epoch_cancellation_until_worker_retirement() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let service = scripted_service(
        ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        15,
        500,
    )
    .expect("scripted generation starts");
    let payload = encoded(&vec![16_384; 12_000]);
    let mut session = session_with_audio(&service, &payload, 8_000);
    let probe = session
        .input
        .as_ref()
        .expect("input exists")
        .take()
        .pcm_budget_probe();
    let blocked_probe = probe.clone();
    let replacement_service = service.clone();

    final_decoder
        .with_next_decode_blocked(
            WAIT,
            || async {
                let item_id = session.commit().expect("item commits").item_id().to_owned();
                (&mut session, item_id)
            },
            |(session, item_id)| async move {
                assert_eq!(blocked_probe.retained_samples(), 8_000);
                cancel_generation_epoch(&replacement_service, session).await;
                session
                    .finish_finalization(&item_id)
                    .await
                    .expect("canceled finalization settles");
                assert_eq!(
                    blocked_probe.retained_samples(),
                    8_000,
                    "final cancellation cannot release worker-owned PCM"
                );
            },
        )
        .await
        .expect("final decode reaches the blocked worker");

    wait_for_decode_retirement(final_decoder, &service).await;
    assert_eq!(probe.retained_samples(), 0);
    drop(session);
    service.shutdown();
}
