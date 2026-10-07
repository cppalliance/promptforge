//! Native Realtime capture: streams the 16 kHz mono 16-bit clip that
//! `PROMPTFORGE_WHISPER_AUDIO` names, or `jfk.wav`, through the mounted
//! `/v1/realtime` route at real-time pace and records every server event it
//! receives, for measuring what a dictation shows and when.
//!
//! The session negotiates the hypothesis and ranges includes the way
//! Workshop does, and the gateway loads the packaged interim and final
//! `[[stt_model]]` pair at its default `[stt]` window and interval. The clip
//! is appended in 100 ms chunks on a wall-clock schedule, and the commit
//! follows the last chunk.
//!
//! With `PROMPTFORGE_REALTIME_CAPTURE` naming a JSON file outside every
//! `fixtures` directory in the repository, the capture writes there each
//! event with its receive time in milliseconds since the first append, the
//! commit's send time, and the completed transcript. The test checks only
//! that the take completes, every frame decodes, and every hypothesis
//! partitions its transcript: it measures and asserts no transcript.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use super::native::{native_clip_24khz, native_speech_service_with};
use super::{Socket, audio_samples, connect, server};

const CAPTURE_VARIABLE: &str = "PROMPTFORGE_REALTIME_CAPTURE";
const HYPOTHESIS_INCLUDE: &str = "item.input_audio_transcription.hypothesis";
const RANGES_INCLUDE: &str = "item.input_audio_transcription.hypothesis.ranges";
const HYPOTHESIS: &str = "conversation.item.input_audio_transcription.hypothesis";
const COMPLETED: &str = "conversation.item.input_audio_transcription.completed";
const FAILED: &str = "conversation.item.input_audio_transcription.failed";
const WIRE_RATE: u64 = 24_000;
/// 100 ms of 24 kHz audio, one append.
const CHUNK_SAMPLES: usize = 2_400;
const CHUNK_MS: u64 = 100;
const CHUNK: Duration = Duration::from_millis(CHUNK_MS);
/// The longest the route may stay silent, as it may through a final decode
/// or the commit's finalization, before the capture stops waiting.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);

type Sink = SplitSink<Socket, Message>;

/// The server half of the socket and every event it delivered, stamped on
/// arrival.
struct Recorder {
    stream: SplitStream<Socket>,
    received: Vec<(Instant, Value)>,
}

impl Recorder {
    /// Records the next server event, or returns `None` once the socket
    /// closes or stays idle past [`IDLE_TIMEOUT`].
    async fn next(&mut self) -> Option<Value> {
        loop {
            let message = tokio::time::timeout(IDLE_TIMEOUT, self.stream.next())
                .await
                .ok()??
                .ok()?;
            let at = Instant::now();
            let text = match message {
                Message::Text(text) => text,
                Message::Close(_) => return None,
                Message::Binary(_) => panic!("Realtime server frames are JSON text"),
                _ => continue,
            };
            let event: Value =
                serde_json::from_str(text.as_str()).expect("Realtime server frame is JSON");
            assert!(
                event["type"].is_string(),
                "every server event names its type: {event}"
            );
            self.received.push((at, event.clone()));
            return Some(event);
        }
    }

    async fn expect(&mut self, expected: &str) -> Value {
        let event = self
            .next()
            .await
            .unwrap_or_else(|| panic!("{expected} arrives"));
        assert_eq!(event["type"], expected, "{event}");
        event
    }

    /// Records events until the take completes or fails, the socket
    /// closes, or the route goes idle.
    async fn until_result(mut self) -> Self {
        while let Some(event) = self.next().await {
            if event["type"] == COMPLETED || event["type"] == FAILED {
                break;
            }
        }
        self
    }

    fn completed(&self) -> Option<Value> {
        self.received
            .iter()
            .find(|(_, event)| event["type"] == COMPLETED)
            .map(|(_, event)| event["transcript"].clone())
    }
}

/// When the client sent its first and last appends and its commit.
struct Schedule {
    first_append: Instant,
    last_append: Instant,
    commit: Instant,
}

async fn send(sink: &mut Sink, event: &Value) {
    sink.send(Message::Text(event.to_string().into()))
        .await
        .expect("client event sends");
}

/// Requests hypotheses with ranges in the session update Workshop sends.
async fn negotiate(sink: &mut Sink, recorder: &mut Recorder) {
    recorder.expect("session.created").await;
    send(
        sink,
        &json!({
            "type": "session.update",
            "event_id": "capture-session",
            "session": {
                "type": "transcription",
                "audio": {"input": {
                    "format": {"type": "audio/pcm", "rate": WIRE_RATE},
                    "noise_reduction": null,
                    "transcription": {"model": "realtime-transcribe", "prompt": ""},
                    "turn_detection": null
                }},
                "include": [HYPOTHESIS_INCLUDE, RANGES_INCLUDE]
            }
        }),
    )
    .await;
    let updated = recorder.expect("session.updated").await;
    assert_eq!(
        updated["session"]["include"],
        json!([HYPOTHESIS_INCLUDE, RANGES_INCLUDE]),
        "the route negotiates hypotheses with ranges"
    );
}

/// Appends `clip` one chunk per [`CHUNK`] of wall time, then commits.
async fn stream_and_commit(sink: &mut Sink, clip: &[i16]) -> Schedule {
    let first_append = Instant::now();
    let mut last_append = first_append;
    for (index, chunk) in (0_u32..).zip(clip.chunks(CHUNK_SAMPLES)) {
        tokio::time::sleep_until(first_append + CHUNK * index).await;
        last_append = Instant::now();
        send(
            sink,
            &json!({
                "type": "input_audio_buffer.append",
                "event_id": format!("append-{index}"),
                "audio": audio_samples(chunk)
            }),
        )
        .await;
    }
    let commit = Instant::now();
    send(
        sink,
        &json!({"type": "input_audio_buffer.commit", "event_id": "commit"}),
    )
    .await;
    Schedule {
        first_append,
        last_append,
        commit,
    }
}

/// Milliseconds from `origin` to `at`, negative for an event before it.
fn signed_ms(origin: Instant, at: Instant) -> i64 {
    let ms = |duration: Duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    if at >= origin {
        ms(at - origin)
    } else {
        -ms(origin - at)
    }
}

fn capture_json(clip: &[i16], schedule: &Schedule, recorder: &Recorder) -> Value {
    let origin = schedule.first_append;
    let samples = u64::try_from(clip.len()).expect("the clip length fits u64");
    json!({
        "audio_ms": samples * 1_000 / WIRE_RATE,
        "chunk_ms": CHUNK_MS,
        "last_append_ms": signed_ms(origin, schedule.last_append),
        "commit_ms": signed_ms(origin, schedule.commit),
        "completed": recorder.completed(),
        "events": recorder
            .received
            .iter()
            .map(|(at, event)| json!({"at_ms": signed_ms(origin, *at), "event": event}))
            .collect::<Vec<_>>(),
    })
}

fn assert_sane(recorder: &Recorder) {
    assert!(
        recorder.completed().as_ref().is_some_and(Value::is_string),
        "the take completes with a transcript: {:?}",
        recorder.received.last()
    );
    for (_, hypothesis) in recorder
        .received
        .iter()
        .filter(|(_, event)| event["type"] == HYPOTHESIS)
    {
        let part = |field: &str| hypothesis[field].as_str().unwrap_or_default().to_owned();
        let parts = format!(
            "{}{}{}",
            part("finalized"),
            part("agreed"),
            part("tentative")
        );
        assert_eq!(
            hypothesis["transcript"].as_str(),
            Some(parts.as_str()),
            "a hypothesis transcript is its finalized, agreed, and tentative parts: {hypothesis}"
        );
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Refuses a capture whose directory resolves inside one of the
/// repository's `fixtures` directories, where a clip's transcript could be
/// committed or replace a fixed input.
fn check_output(output: &Path) -> Result<(), String> {
    let parent = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let resolved = std::fs::canonicalize(parent).map_err(|error| {
        format!(
            "{CAPTURE_VARIABLE} requires an existing directory, but {} does not resolve: {error}",
            parent.display()
        )
    })?;
    let repository = std::fs::canonicalize(repository_root())
        .map_err(|error| format!("the repository root resolves: {error}"))?;
    let in_fixtures = resolved.strip_prefix(&repository).is_ok_and(|relative| {
        relative
            .components()
            .any(|component| component.as_os_str() == "fixtures")
    });
    if in_fixtures {
        return Err(format!(
            "{CAPTURE_VARIABLE} requires a path outside the repository's fixtures directories, but names {}",
            output.display()
        ));
    }
    Ok(())
}

// The client paces and stamps on its own worker, apart from the route's
// task, as a separate Workshop process would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn native_realtime_capture_records_every_server_event_at_real_time_pace() {
    let output = std::env::var_os(CAPTURE_VARIABLE).map(PathBuf::from);
    if let Some(output) = &output {
        check_output(output).unwrap_or_else(|reason| panic!("{reason}"));
    }
    let clip = native_clip_24khz();
    let service = native_speech_service_with("");
    let server = server(true, &service).await;
    let (mut sink, stream) = connect(server.addr, Some("test-token"), None, None)
        .await
        .split();
    let mut recorder = Recorder {
        stream,
        received: Vec::new(),
    };
    negotiate(&mut sink, &mut recorder).await;
    let recording = tokio::spawn(recorder.until_result());
    let schedule = stream_and_commit(&mut sink, &clip).await;
    let recorder = recording.await.expect("the recorder joins");
    sink.close().await.expect("socket closes");
    server.shutdown().await;
    tokio::task::spawn_blocking(move || service.shutdown())
        .await
        .expect("native shutdown thread joins");

    if let Some(output) = &output {
        let capture = capture_json(&clip, &schedule, &recorder);
        let mut text = serde_json::to_string_pretty(&capture).expect("the capture serializes");
        text.push('\n');
        std::fs::write(output, text)
            .unwrap_or_else(|error| panic!("{} writes: {error}", output.display()));
        eprintln!(
            "{}: {} events, completed {:?}",
            output.display(),
            recorder.received.len(),
            recorder.completed()
        );
    }
    assert_sane(&recorder);
}

#[test]
fn capture_inside_a_committed_fixtures_directory_is_refused() {
    let repository = repository_root();
    for output in [
        repository.join("crates/gateway/stt/api/tests/fixtures/replay/capture.json"),
        repository.join("crates/gateway/stt/api/tests/fixtures/realtime/capture.json"),
        repository.join("crates/workshop/server/tests/fixtures/../fixtures/capture.json"),
    ] {
        assert!(
            check_output(&output).is_err(),
            "{} resolves inside a fixtures directory and is refused",
            output.display()
        );
    }
}

#[test]
fn capture_outside_the_fixtures_directories_is_accepted() {
    for output in [
        std::env::temp_dir().join("capture.json"),
        repository_root().join("crates/gateway/capture.json"),
    ] {
        assert_eq!(check_output(&output), Ok(()), "{}", output.display());
    }
}
