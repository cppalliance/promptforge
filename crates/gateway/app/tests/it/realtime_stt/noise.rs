//! Native Realtime noise: streams each 16 kHz mono 16-bit `.wav` clip in
//! the directory `PROMPTFORGE_NOISE_CLIPS` names, the noise clips the native
//! Silero tests read, through the mounted `/v1/realtime` route at real-time
//! pace, as the native capture streams its clip, and asserts that no
//! hypothesis and no final shows text.

use std::path::PathBuf;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;

use super::capture::{Recorder, negotiate, stream_and_commit};
use super::native::{clip_24khz, native_speech_service_with, recording_fallbacks};
use super::{connect, server};

const CLIPS_VARIABLE: &str = "PROMPTFORGE_NOISE_CLIPS";
const MIN_NOISE_CLIPS: usize = 3;
const HYPOTHESIS: &str = "conversation.item.input_audio_transcription.hypothesis";
const DELTA: &str = "conversation.item.input_audio_transcription.delta";
const COMPLETED: &str = "conversation.item.input_audio_transcription.completed";

/// Every `.wav` clip in the directory `PROMPTFORGE_NOISE_CLIPS` names, in
/// path order.
fn noise_clips() -> Vec<PathBuf> {
    let Some(directory) = std::env::var_os(CLIPS_VARIABLE).map(PathBuf::from) else {
        panic!("{CLIPS_VARIABLE} is set");
    };
    let mut clips = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} reads: {error}", directory.display()))
        .map(|entry| entry.expect("a noise clip entry reads").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "wav"))
        .collect::<Vec<_>>();
    clips.sort();
    assert!(
        clips.len() >= MIN_NOISE_CLIPS,
        "{} holds at least {MIN_NOISE_CLIPS} noise clips, found {}",
        directory.display(),
        clips.len()
    );
    clips
}

/// Every event the take showed text in, labelled with its source: a
/// hypothesis decoded from an interim window, a hypothesis a landed final
/// sent with an empty span, a plain delta, or the completed final.
fn shown_text(recorder: &Recorder) -> Vec<String> {
    assert!(
        recorder.completed().as_ref().is_some_and(Value::is_string),
        "the take completes with a transcript"
    );
    recorder
        .events()
        .filter_map(|event| {
            let field = |name: &str| event[name].as_str().unwrap_or_default();
            let (source, text) = match event["type"].as_str()? {
                HYPOTHESIS => (hypothesis_source(event), field("transcript")),
                DELTA => ("delta".to_owned(), field("delta")),
                COMPLETED => ("completed final".to_owned(), field("transcript")),
                _ => return None,
            };
            (!text.trim().is_empty()).then(|| format!("{source}: {text:?}"))
        })
        .collect()
}

fn hypothesis_source(event: &Value) -> String {
    let (start, end) = (&event["audio_start_ms"], &event["audio_end_ms"]);
    let kind = if start == end {
        "landed-final hypothesis"
    } else {
        "interim-window hypothesis"
    };
    let part = |name: &str| event[name].as_str().unwrap_or_default().to_owned();
    format!(
        "{kind} revision {} at {start}..{end} ms (finalized {:?}, agreed {:?}, tentative {:?})",
        event["revision"],
        part("finalized"),
        part("agreed"),
        part("tentative")
    )
}

// The client paces on its own worker, apart from the route's task, as the
// native capture does.
#[test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
fn native_realtime_shows_no_text_for_noise_clips() {
    recording_fallbacks(|fallbacks| async move {
        let clips = noise_clips();
        let (service, cache) = native_speech_service_with("");
        let server = server(true, &service).await;
        let mut shown = Vec::new();
        for clip in &clips {
            let audio = clip_24khz(clip);
            let (mut sink, stream) = connect(server.addr, Some("test-token"), None, None)
                .await
                .split();
            let mut recorder = Recorder::new(stream);
            negotiate(&mut sink, &mut recorder).await;
            let recording = tokio::spawn(recorder.until_result());
            stream_and_commit(&mut sink, &audio).await;
            let recorder = recording.await.expect("the recorder joins");
            sink.close().await.expect("socket closes");
            shown.extend(
                shown_text(&recorder)
                    .into_iter()
                    .map(|text| format!("{}: {text}", clip.display())),
            );
        }
        server.shutdown().await;
        tokio::task::spawn_blocking(move || service.shutdown())
            .await
            .expect("native shutdown thread joins");
        drop(cache);

        fallbacks.assert_none();
        assert!(
            shown.is_empty(),
            "noise without speech shows text:\n{}",
            shown.join("\n")
        );
    });
}
