//! Tests for interim decodes that follow the detector's speech: each window
//! ends at the speech tail, and a window holding no new speech is skipped.
//! Also tests the catch-up decode that the completion pass reaping a slow
//! decode schedules for a tick that found it in flight.

use std::ops::Range;
use std::time::Duration;

use base64::Engine as _;
use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedDetector, ScriptedModelFactory};
use serde_json::Value;

use super::{HYPOTHESIS, Session, sample_millis};
use crate::SpeechService;
use crate::realtime::registry::SessionRegistry;
use crate::segment::FRAME_SAMPLES;
use crate::take::SPEECH_TAIL_SAMPLES;
use crate::test_fixtures::scripted_service;

const WAIT: Duration = Duration::from_secs(5);

const fn at(frames: usize) -> u64 {
    (frames * FRAME_SAMPLES) as u64
}

/// A hypothesis session on a scripted generation without a final pass,
/// whose take hears speech only where its detector's script says.
struct GatedSession {
    service: SpeechService,
    session: Session,
    interim: ScriptedDecoder,
    detector: ScriptedDetector,
    appended: u64,
}

impl GatedSession {
    /// `transcripts` are the interim decodes' outputs, in order.
    fn new(speech: &[Range<u64>], transcripts: &[&str]) -> Self {
        let interim = ScriptedDecoder::new();
        for transcript in transcripts {
            interim.push_text(*transcript);
        }
        let service = scripted_service(ScriptedModelFactory::new(interim.clone()), 15, 500)
            .expect("scripted generation starts");
        let engine = service
            .state
            .active()
            .expect("scripted generation is active");
        let registration = SessionRegistry::default()
            .register()
            .expect("session registers");
        let mut session = Session::new(registration, Some(engine));
        session
            .update_text(
                &serde_json::json!({
                    "type": "session.update",
                    "session": {"type": "transcription", "include": [HYPOTHESIS]}
                })
                .to_string(),
            )
            .expect("hypothesis include applies");
        let runs = speech.iter().map(|run| {
            let [start, end] = [run.start, run.end]
                .map(|sample| usize::try_from(sample).expect("a test sample fits usize"));
            (start, end)
        });
        Self {
            service,
            session,
            interim,
            detector: ScriptedDetector::new(runs),
            appended: 0,
        }
    }

    /// Appends silent audio until the take holds `through` samples, so
    /// only the detector decides speech. Three 24 kHz samples resample to
    /// two, so an even sample count arrives exactly.
    fn append_through(&mut self, through: u64) {
        let input_samples =
            usize::try_from((through - self.appended) / 2 * 3).expect("a test append fits usize");
        let payload =
            base64::engine::general_purpose::STANDARD.encode(vec![0_u8; input_samples * 2]);
        self.session
            .append_base64_detecting(&payload, Box::new(self.detector.clone()))
            .expect("audio appends");
        self.appended = through;
    }

    /// Appends through `through` and runs one interim tick and the
    /// completion pass that reaps its decode. Returns the decoded window's
    /// span in milliseconds, or `None` when the take skipped the decode.
    async fn tick(&mut self, through: u64) -> Option<(u64, u64)> {
        self.append_through(through);
        let decodes = self.interim.requests().len();
        self.session
            .schedule_interim()
            .expect("the interim schedules");
        let event = self.reap().await;
        let requests = self.interim.requests();
        let [request] = requests.get(decodes..).unwrap_or_default() else {
            assert!(event.is_none(), "an event without a decode: {event:?}");
            return None;
        };
        let (start, end) = span(event.as_ref()).expect("every scripted transcript is shown");
        assert_eq!(
            sample_millis(request.samples().len() as u64),
            end - start,
            "the hypothesis spans the decoded window"
        );
        Some((start, end))
    }

    /// Runs an interim tick that finds a decode in flight.
    fn missed_tick(&mut self) {
        self.session.schedule_interim().expect("the tick runs");
    }

    /// Appends through `through` and starts a decode that the decoder holds
    /// while `during` runs, so it outlasts any tick `during` runs. Returns
    /// the update of the completion pass that reaps it.
    async fn slow_decode(&mut self, through: u64, during: impl FnOnce(&mut Self)) -> Option<Value> {
        self.append_through(through);
        let interim = self.interim.clone();
        let gated = interim
            .with_next_decode_blocked(
                WAIT,
                move || async move {
                    self.session
                        .schedule_interim()
                        .expect("the interim schedules");
                    self
                },
                move |gated| async move {
                    during(gated);
                    gated
                },
            )
            .await
            .expect("the decode reaches the blocked decoder");
        gated.reap().await
    }

    /// Runs the socket loop's completion pass on the in-flight decode: it
    /// reaps the decode, then catches up a tick that found it in flight.
    async fn reap(&mut self) -> Option<Value> {
        let event = self
            .session
            .finish_interim()
            .await
            .expect("the interim completes");
        self.session
            .catch_up_interim()
            .expect("the catch-up schedules");
        event.map(|event| serde_json::to_value(event).expect("the update serializes"))
    }

    fn shutdown(self) {
        drop(self.session);
        self.service.shutdown();
    }
}

/// The span in milliseconds of a window from the take start to `end`.
fn window_to(end: u64) -> (u64, u64) {
    (0, sample_millis(end))
}

/// A gated session whose detector hears speech throughout.
fn speaking(transcripts: &[&str]) -> GatedSession {
    GatedSession::new(std::slice::from_ref(&(0..at(200))), transcripts)
}

/// The audio span in milliseconds that a hypothesis reports.
fn span(event: Option<&Value>) -> Option<(u64, u64)> {
    let event = event?;
    let field = |name: &str| event[name].as_u64().expect("the hypothesis spans audio");
    Some((field("audio_start_ms"), field("audio_end_ms")))
}

#[tokio::test]
async fn interim_decodes_follow_speech_through_its_tail_stop_in_silence_and_resume_with_speech() {
    let mut gated = GatedSession::new(
        &[0..at(40), at(100)..at(130)],
        &[
            "alpha",
            "alpha beta",
            "alpha beta",
            "alpha beta",
            "alpha beta gamma",
        ],
    );

    assert_eq!(
        gated.tick(at(30)).await,
        Some(window_to(at(30))),
        "speech is decoded however quiet its audio"
    );
    assert_eq!(gated.tick(at(40)).await, Some(window_to(at(40))));
    assert_eq!(
        gated.tick(at(40) + SPEECH_TAIL_SAMPLES / 2).await,
        Some(window_to(at(40) + SPEECH_TAIL_SAMPLES / 2)),
        "a window ends at the buffer end while the speech tail is still arriving"
    );
    assert_eq!(
        gated.tick(at(60)).await,
        Some(window_to(at(40) + SPEECH_TAIL_SAMPLES)),
        "the completed tail is decoded once more, and audio_end_ms reports the window end"
    );
    assert_eq!(gated.tick(at(70)).await, None, "silence decodes nothing");
    assert_eq!(gated.tick(at(95)).await, None, "silence decodes nothing");
    assert_eq!(
        gated.tick(at(110)).await,
        Some(window_to(at(110))),
        "new speech resumes decoding"
    );
    gated.shutdown();
}

#[tokio::test]
async fn a_take_that_has_heard_no_speech_decodes_nothing() {
    let mut gated = GatedSession::new(&[], &["you"]);
    assert_eq!(gated.tick(at(60)).await, None);
    assert_eq!(gated.tick(at(120)).await, None);
    gated.shutdown();
}

#[tokio::test]
async fn a_tick_during_a_slow_decode_starts_the_next_decode_in_the_pass_that_reaps_it() {
    let mut gated = speaking(&["alpha", "alpha beta"]);
    let slow = gated
        .slow_decode(at(30), |gated| {
            gated.append_through(at(40));
            gated.missed_tick();
        })
        .await;
    assert_eq!(span(slow.as_ref()), Some(window_to(at(30))));
    assert!(
        gated.session.interim_task.is_some(),
        "the next decode starts before another tick"
    );
    gated.append_through(at(50));
    let caught_up = gated.reap().await;
    assert_eq!(
        span(caught_up.as_ref()),
        Some(window_to(at(40))),
        "the catch-up decodes the speech that arrived during the slow decode"
    );
    assert!(
        gated.session.interim_task.is_none(),
        "no tick found the catch-up in flight, so reaping it schedules nothing"
    );
    assert_eq!(gated.interim.requests().len(), 2);
    gated.shutdown();
}

#[tokio::test]
async fn reaping_a_slow_decode_that_no_tick_found_in_flight_schedules_nothing() {
    let mut gated = speaking(&["alpha"]);
    gated
        .slow_decode(at(30), |gated| gated.append_through(at(40)))
        .await;
    assert!(
        gated.session.interim_task.is_none(),
        "the speech that arrived during the decode waits for the next tick"
    );
    assert_eq!(gated.interim.requests().len(), 1);
    gated.shutdown();
}

#[tokio::test]
async fn a_catch_up_whose_window_has_not_advanced_is_skipped() {
    let mut gated = speaking(&["alpha", "alpha beta"]);
    gated.slow_decode(at(30), GatedSession::missed_tick).await;
    assert!(
        gated.session.interim_task.is_none(),
        "no speech arrived after the slow decode's window"
    );
    assert_eq!(gated.interim.requests().len(), 1);
    gated
        .slow_decode(at(40), |gated| gated.append_through(at(50)))
        .await;
    assert!(
        gated.session.interim_task.is_none(),
        "the skipped catch-up leaves no missed tick for a later decode"
    );
    assert_eq!(gated.interim.requests().len(), 2);
    gated.shutdown();
}

#[tokio::test]
async fn the_catch_up_after_an_overloaded_decode_retries_its_window() {
    let mut gated = speaking(&[]);
    gated.interim.push_overloaded();
    gated.interim.push_text("alpha");
    let overloaded = gated.slow_decode(at(30), GatedSession::missed_tick).await;
    assert_eq!(overloaded, None, "the overloaded decode shows nothing");
    assert!(
        gated.session.interim_task.is_some(),
        "the window the full worker queue never decoded is retried at once"
    );
    let retried = gated.reap().await;
    assert_eq!(span(retried.as_ref()), Some(window_to(at(30))));
    let windows = gated
        .interim
        .requests()
        .iter()
        .map(|request| request.samples().len())
        .collect::<Vec<_>>();
    assert_eq!(
        windows,
        [usize::try_from(at(30)).expect("a test window fits usize"); 2],
        "the catch-up decodes the overloaded decode's window"
    );
    gated.shutdown();
}
