//! Tests for final-segment handoff, pipeline completion, and queue capacity.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::{FINAL_SEGMENT_CAPACITY, FinalCommand, FinalPipeline, run_final_pipeline};
use crate::take::state::TakeState;
use crate::take::window::AcceptedHypothesis;
use crate::take::{Take, TakeFailure};

fn accepted_from_snapshot(
    take: &Take,
    range: std::ops::Range<u64>,
    text: &str,
    committed_samples: u64,
) -> Vec<AcceptedHypothesis> {
    take.next_window_snapshot(text, range.start, range.start, range.end)
        .expect("the production window snapshot is accepted");
    TakeState::lock(&take.whole_window).accepted_hypotheses(committed_samples)
}

#[tokio::test]
async fn natural_handoff_stays_resident_until_ordered_pipeline_transfer() {
    let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(0));
    let pipeline = FinalPipeline {
        commands,
        task: tokio::spawn(std::future::pending()),
        pending_segments: Arc::clone(&pending),
    };
    let state = TakeState::default();
    let mut samples = vec![0.5; 16_000];
    samples.extend(vec![0.0; 48_000]);
    TakeState::lock(&state.buffer)
        .append(samples)
        .expect("resident PCM reserves");

    pipeline.submit_closed_segments(&state);

    let command = receiver.try_recv().expect("closed segment queues");
    let FinalCommand::Segment { range, owner, .. } = command else {
        panic!("ordinary speech queues a final decode");
    };
    let resident = TakeState::lock(&state.buffer);
    assert_eq!(resident.origin(), 0);
    assert!(resident.end() >= range.end);
    assert_eq!(pending.load(Ordering::Acquire), 1);
    drop(resident);
    drop(owner);
    assert_eq!(pending.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn saturated_handoff_does_not_compact_unowned_pcm() {
    let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(FINAL_SEGMENT_CAPACITY));
    let pipeline = FinalPipeline {
        commands,
        task: tokio::spawn(std::future::pending()),
        pending_segments: pending,
    };
    let state = TakeState::default();
    let mut samples = vec![0.5; 16_000];
    samples.extend(vec![0.0; 48_000]);
    TakeState::lock(&state.buffer)
        .append(samples)
        .expect("resident PCM reserves");

    pipeline.submit_closed_segments(&state);

    assert!(receiver.try_recv().is_err());
    assert_eq!(TakeState::lock(&state.buffer).origin(), 0);
    assert!(matches!(
        state.pending_failure().as_deref(),
        Some(TakeFailure::SegmentCapacity)
    ));
}

#[tokio::test]
async fn finalization_reports_a_typed_failure_after_the_pipeline_exits() {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    drop(receiver);
    let pipeline = FinalPipeline {
        commands,
        task: tokio::spawn(std::future::pending()),
        pending_segments: Arc::new(AtomicUsize::new(0)),
    };

    let failure = pipeline
        .finalization(0, Vec::new())
        .await
        .expect_err("an exited pipeline fails the finalization");
    assert!(matches!(&*failure, TakeFailure::PipelineExited));
}

#[tokio::test]
async fn pipeline_reconciles_short_tail_without_decoding_it() {
    let (commands, receiver) = mpsc::channel(1);
    let take = Take::without_final(Vec::new());
    take.append(vec![0.5; 4_800]).expect("tail PCM reserves");
    let accepted = accepted_from_snapshot(&take, 0..4_800, "last word", 4_800);
    let state = Arc::clone(&take.state);
    let whole_window = Arc::clone(&take.whole_window);
    let calls = Arc::new(AtomicUsize::new(0));
    let decode_calls = Arc::clone(&calls);
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        state,
        whole_window,
        move |_| {
            decode_calls.fetch_add(1, Ordering::SeqCst);
            async { Some(Ok("must not decode".to_owned())) }
        },
    ));
    let (reply, completion) = oneshot::channel();
    commands
        .send(FinalCommand::Complete {
            committed_samples: 4_800,
            accepted,
            reply,
        })
        .await
        .expect("completion queues");

    assert_eq!(
        completion
            .await
            .expect("completion replies")
            .expect("completion succeeds"),
        "last word"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.await.expect("pipeline exits");
}

#[tokio::test]
async fn pipeline_rejects_a_hypothesis_with_only_partial_skipped_coverage() {
    let (commands, receiver) = mpsc::channel(1);
    let take = Take::without_final(Vec::new());
    take.append(vec![0.5; 8_000]).expect("tail PCM reserves");
    let accepted = accepted_from_snapshot(&take, 0..8_000, "must not inherit", 8_000);
    TakeState::lock(&take.state.segmenter).set_consumed_for_test(4_000);
    let state = Arc::clone(&take.state);
    let whole_window = Arc::clone(&take.whole_window);
    let calls = Arc::new(AtomicUsize::new(0));
    let decode_calls = Arc::clone(&calls);
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        state,
        whole_window,
        move |_| {
            decode_calls.fetch_add(1, Ordering::SeqCst);
            async { Some(Ok("must not decode".to_owned())) }
        },
    ));
    let (reply, completion) = oneshot::channel();
    commands
        .send(FinalCommand::Complete {
            committed_samples: 8_000,
            accepted,
            reply,
        })
        .await
        .expect("completion queues");

    assert_eq!(
        completion
            .await
            .expect("completion replies")
            .expect("completion succeeds"),
        String::new()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.await.expect("pipeline exits");
}

#[tokio::test]
async fn characterize_full_final_queue_fails_the_take_with_segment_capacity() {
    // The owner reservation gate fills at `FINAL_SEGMENT_CAPACITY`; a
    // one-slot channel fills first and reaches the `try_send` gate instead.
    for channel_capacity in [FINAL_SEGMENT_CAPACITY, 1] {
        let (commands, mut receiver) = mpsc::channel(channel_capacity);
        let pipeline = FinalPipeline {
            commands,
            task: tokio::spawn(std::future::pending()),
            pending_segments: Arc::new(AtomicUsize::new(0)),
        };
        let state = TakeState::default();
        for _ in 0..=FINAL_SEGMENT_CAPACITY {
            let mut samples = vec![0.5; 16_000];
            samples.extend(vec![0.0; 48_000]);
            TakeState::lock(&state.buffer)
                .append(samples)
                .expect("resident PCM reserves");
        }

        pipeline.submit_closed_segments(&state);

        let mut queued = 0;
        while receiver.try_recv().is_ok() {
            queued += 1;
        }
        assert_eq!(
            queued, channel_capacity,
            "channel capacity {channel_capacity}"
        );
        assert!(
            matches!(
                state.pending_failure().as_deref(),
                Some(TakeFailure::SegmentCapacity)
            ),
            "channel capacity {channel_capacity}"
        );
    }
}
