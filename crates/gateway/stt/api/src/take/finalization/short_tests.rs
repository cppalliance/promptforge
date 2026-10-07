//! Tests for speech runs shorter than the final window: a click the segmenter
//! skips and a short word the final pass decodes.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use gateway_stt_engine::DecodeRequest;
use tokio::sync::mpsc;

use super::{FINAL_SEGMENT_CAPACITY, FinalPipeline, run_final_pipeline};
use crate::take::Take;

/// Appends `samples` of loud speech or digital silence and submits whatever
/// segments they close.
fn hear(take: &Take, loud: bool, samples: usize) {
    take.append(vec![if loud { 0.5 } else { 0.0 }; samples])
        .expect("the audio appends");
    take.submit_closed_segments();
}

async fn settle(take: &Take) {
    while take.pending_final_segments() > 0 {
        tokio::task::yield_now().await;
    }
}

/// A take whose final decodes return `outputs` in order and report each
/// request's sample count and history.
fn scripted_take(outputs: &[&'static str]) -> (Take, mpsc::UnboundedReceiver<(usize, String)>) {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let mut take = Take::new(Vec::new(), None);
    let (report, requests) = mpsc::unbounded_channel();
    let mut outputs = outputs.iter().copied().collect::<VecDeque<_>>();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        Arc::clone(&take.state),
        Arc::clone(&take.whole_window),
        move |request: DecodeRequest| {
            let _ = report.send((request.samples().len(), request.finalized().to_owned()));
            let text = outputs.pop_front().unwrap_or_default().to_owned();
            async move { Some(Ok(text)) }
        },
    ));
    take.final_pipeline = Some(FinalPipeline {
        commands,
        task,
        pending_segments: Arc::new(AtomicUsize::new(0)),
    });
    (take, requests)
}

#[tokio::test]
async fn a_click_keeps_its_accepted_text_when_decoded_speech_follows() {
    let (take, mut requests) = scripted_take(&["The commit rule."]);

    hear(&take, false, 16_384);
    hear(&take, true, 3_584);
    hear(&take, false, 4_800);
    take.next_window_snapshot("Hey.", &[], 0, 0, 24_768)
        .expect("the word is accepted from a window that ends in its closing silence");
    hear(&take, false, 4_928);
    settle(&take).await;
    let consumed = take.consumed();
    assert_eq!(
        consumed, 21_568,
        "the 224 ms click closed as a skipped segment"
    );
    assert_eq!(
        take.finalized(),
        "Hey.",
        "the skipped segment keeps its accepted text"
    );

    hear(&take, true, 16_000);
    take.next_window_snapshot("The commit rule.", &[], consumed, consumed, 45_696)
        .expect("the next sentence is accepted");
    let completed = take
        .finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds");
    assert_eq!(completed, "Hey. The commit rule.");
    assert_eq!(
        requests.try_recv().map(|(_, history)| history).as_deref(),
        Ok(""),
        "interim text conditions no final decode, since whisper copies its prompt's style"
    );
}

#[tokio::test]
async fn a_short_word_before_silence_settles_its_punctuated_final_over_the_interim_word() {
    let (take, mut requests) = scripted_take(&["Hey.", "The commit rule."]);

    hear(&take, false, 16_384);
    hear(&take, true, 6_656);
    hear(&take, false, 4_800);
    take.next_window_snapshot("Hey", &[], 0, 0, 27_840)
        .expect("the unpunctuated interim word is accepted");
    hear(&take, false, 27_456);
    settle(&take).await;
    let consumed = take.consumed();
    assert_eq!(
        consumed, 24_640,
        "the 416 ms word closed after two seconds of silence"
    );
    assert_eq!(
        requests.try_recv(),
        Ok((16_256, String::new())),
        "the final pass decodes the word with its pre-roll and hangover"
    );
    let (snapshot, _) = take
        .refreshed_snapshot(0)
        .expect("the landed final refreshes the snapshot");
    let (transcript, finalized, _, _) = snapshot.into_parts();
    assert_eq!(
        (transcript.as_str(), finalized.as_str()),
        ("Hey.", "Hey."),
        "the punctuated final replaces the interim word"
    );

    hear(&take, true, 16_000);
    take.next_window_snapshot("the commit rule", &[], consumed, consumed, 71_296)
        .expect("the next sentence is accepted");
    let completed = take
        .finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds");
    assert_eq!(
        completed, "Hey. The commit rule.",
        "the word completes once"
    );
    assert_eq!(
        requests.try_recv().map(|(_, history)| history).as_deref(),
        Ok("Hey."),
        "the word's final conditions the next final decode"
    );
}
