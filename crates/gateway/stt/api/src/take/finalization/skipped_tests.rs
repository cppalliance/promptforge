//! Tests for a segment too short to decode that the segmenter skips.

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

#[tokio::test]
async fn a_word_too_short_to_decode_keeps_its_text_when_decoded_speech_follows() {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let mut take = Take::new(Vec::new(), None);
    let (report, mut prompts) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        Arc::clone(&take.state),
        Arc::clone(&take.whole_window),
        move |request: DecodeRequest| {
            let _ = report.send(request.finalized().to_owned());
            async { Some(Ok("The commit rule.".to_owned())) }
        },
    ));
    take.final_pipeline = Some(FinalPipeline {
        commands,
        task,
        pending_segments: Arc::new(AtomicUsize::new(0)),
    });

    hear(&take, false, 16_000);
    hear(&take, true, 6_720);
    hear(&take, false, 6_080);
    take.next_window_snapshot("Hey.", &[], 0, 0, 28_800)
        .expect("the word is accepted from a window that ends in its closing silence");
    hear(&take, false, 4_800);
    settle(&take).await;
    let consumed = take.consumed();
    assert_eq!(
        consumed, 24_640,
        "the 450 ms word closed as a skipped segment"
    );
    assert_eq!(
        take.finalized(),
        "Hey.",
        "the skipped segment keeps its accepted text"
    );

    hear(&take, true, 16_000);
    take.next_window_snapshot("The commit rule.", &[], consumed, consumed, 49_600)
        .expect("the next sentence is accepted");
    let completed = take
        .finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds");
    assert_eq!(completed, "Hey. The commit rule.");
    assert_eq!(
        prompts.try_recv().as_deref(),
        Ok(""),
        "interim text conditions no final decode, since whisper copies its prompt's style"
    );
}
