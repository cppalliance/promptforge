//! Tests for interim decodes that follow the detector's speech: each window
//! ends at the speech tail, and a window holding no new speech is skipped.

use std::ops::Range;

use base64::Engine as _;
use gateway_stt_engine::FallbackDetector;
use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedDetector, ScriptedModelFactory};

use super::{HYPOTHESIS, Session, sample_millis};
use crate::SpeechService;
use crate::realtime::registry::SessionRegistry;
use crate::segment::FRAME_SAMPLES;
use crate::take::SPEECH_TAIL_SAMPLES;
use crate::test_fixtures::scripted_service;

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
            .append_base64_detecting(
                &payload,
                FallbackDetector::new(Box::new(self.detector.clone())),
            )
            .expect("audio appends");
        self.appended = through;
    }

    /// Appends through `through` and runs one interim tick. Returns the
    /// decoded window's span in milliseconds, or `None` when the take
    /// skipped the decode.
    async fn tick(&mut self, through: u64) -> Option<(u64, u64)> {
        self.append_through(through);
        let decodes = self.interim.requests().len();
        self.session
            .schedule_interim()
            .expect("the interim schedules");
        let event = self
            .session
            .finish_interim()
            .await
            .expect("the interim completes");
        let requests = self.interim.requests();
        let [request] = requests.get(decodes..).unwrap_or_default() else {
            assert!(event.is_none(), "an event without a decode: {event:?}");
            return None;
        };
        let event = serde_json::to_value(event.expect("every scripted transcript is shown"))
            .expect("hypothesis serializes");
        let span = |field: &str| event[field].as_u64().expect("the hypothesis spans audio");
        let (start, end) = (span("audio_start_ms"), span("audio_end_ms"));
        assert_eq!(
            sample_millis(request.samples().len() as u64),
            end - start,
            "the hypothesis spans the decoded window"
        );
        Some((start, end))
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
        gated.tick(at(44)).await,
        Some(window_to(at(44))),
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
