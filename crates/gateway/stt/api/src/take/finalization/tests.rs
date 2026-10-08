//! Tests for final-segment handoff, pipeline completion, and queue capacity.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gateway_stt_engine::DecodeRequest;
use tokio::sync::{mpsc, oneshot};

use super::{
    ClosedRange, FINAL_SEGMENT_CAPACITY, FinalCommand, FinalPipeline, FinalSegmentOwner,
    run_final_pipeline,
};
use crate::take::state::TakeState;
use crate::take::window::AcceptedHypothesis;
use crate::take::{Take, TakeFailure};

pub(super) fn queue(
    commands: mpsc::Sender<FinalCommand>,
    task: tokio::task::JoinHandle<()>,
    pending_segments: Arc<AtomicUsize>,
) -> FinalPipeline {
    FinalPipeline {
        commands,
        task,
        pending_segments,
    }
}

/// One second of speech followed by three of silence, which closes one
/// natural segment.
pub(super) fn closed_speech() -> Vec<f32> {
    let mut samples = Vec::with_capacity(64_000);
    samples.resize(16_000, 0.5);
    samples.resize(64_000, 0.0);
    samples
}

/// Appends `samples` and classifies them for segment closing, as a take with
/// a final pipeline does.
fn hear(state: &TakeState, samples: Vec<f32>) {
    TakeState::lock(&state.buffer)
        .append(samples)
        .expect("resident PCM reserves");
    state.classify(true).expect("classified");
}

pub(super) fn saturate(pending: &Arc<AtomicUsize>) -> Vec<FinalSegmentOwner> {
    (0..FINAL_SEGMENT_CAPACITY)
        .map(|_| FinalSegmentOwner::reserve(pending).expect("the queue has a free slot"))
        .collect()
}

fn accepted_from_snapshot(
    take: &Take,
    range: std::ops::Range<u64>,
    text: &str,
    committed_samples: u64,
) -> Vec<AcceptedHypothesis> {
    take.next_window_snapshot(text, &[], range.start, range.start, range.end)
        .expect("the production window snapshot is accepted");
    TakeState::lock(&take.whole_window).accepted_hypotheses(committed_samples)
}

#[tokio::test]
async fn natural_handoff_stays_resident_until_ordered_pipeline_transfer() {
    let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(0));
    let pipeline = queue(
        commands,
        tokio::spawn(std::future::pending()),
        Arc::clone(&pending),
    );
    let state = TakeState::default();
    hear(&state, closed_speech());

    pipeline.submit_closed_segments(&state);

    let command = receiver.try_recv().expect("closed segment queues");
    let FinalCommand::Closed {
        closed: ClosedRange::Segment { range, .. },
        owner,
    } = command
    else {
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
async fn saturated_handoff_holds_the_closed_range_resident_without_failing() {
    let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(0));
    let pipeline = queue(
        commands,
        tokio::spawn(std::future::pending()),
        Arc::clone(&pending),
    );
    let held = saturate(&pending);
    let state = TakeState::default();
    hear(&state, closed_speech());

    pipeline.submit_closed_segments(&state);

    assert!(receiver.try_recv().is_err());
    assert_eq!(TakeState::lock(&state.buffer).origin(), 0);
    assert!(state.pending_failure().is_none());
    drop(held);
    pipeline.submit_closed_segments(&state);
    assert!(
        matches!(
            receiver.try_recv(),
            Ok(FinalCommand::Closed {
                closed: ClosedRange::Segment { .. },
                ..
            })
        ),
        "the held range queues once a slot frees"
    );
}

#[tokio::test]
async fn finalization_reports_a_typed_failure_after_the_pipeline_exits() {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    drop(receiver);
    let pipeline = queue(
        commands,
        tokio::spawn(std::future::pending()),
        Arc::new(AtomicUsize::new(0)),
    );

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
async fn a_full_final_queue_holds_later_ranges_in_order_without_failing_the_take() {
    // The owner reservation gate fills at `FINAL_SEGMENT_CAPACITY`; a
    // one-slot channel fills first and reaches the `try_send` gate instead.
    for channel_capacity in [FINAL_SEGMENT_CAPACITY, 1] {
        let (commands, mut receiver) = mpsc::channel(channel_capacity);
        let pipeline = queue(
            commands,
            tokio::spawn(std::future::pending()),
            Arc::new(AtomicUsize::new(0)),
        );
        let state = TakeState::default();
        for _ in 0..=FINAL_SEGMENT_CAPACITY {
            hear(&state, closed_speech());
        }

        pipeline.submit_closed_segments(&state);

        let mut queued = Vec::new();
        while let Ok(command) = receiver.try_recv() {
            queued.push(command);
        }
        assert_eq!(
            queued.len(),
            channel_capacity,
            "channel capacity {channel_capacity}"
        );
        assert!(
            state.pending_failure().is_none(),
            "channel capacity {channel_capacity}"
        );
        assert_eq!(TakeState::lock(&state.buffer).origin(), 0);
        drop(queued);

        let mut ends = Vec::new();
        loop {
            pipeline.submit_closed_segments(&state);
            let Ok(FinalCommand::Closed {
                closed: ClosedRange::Segment { range, .. },
                ..
            }) = receiver.try_recv()
            else {
                break;
            };
            ends.push(range.end);
        }
        assert_eq!(
            ends.len(),
            FINAL_SEGMENT_CAPACITY + 1 - channel_capacity,
            "every held range retries, channel capacity {channel_capacity}"
        );
        assert!(ends.is_sorted(), "held ranges retry in audio order");
    }
}

#[tokio::test]
async fn a_held_range_decodes_once_the_final_queue_has_room() {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(0));
    let mut take = Take::without_final(Vec::new());
    let (report, mut decodes) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        Arc::clone(&take.state),
        Arc::clone(&take.whole_window),
        move |request: DecodeRequest| {
            let _ = report.send(request.samples().len());
            async { Some(Ok("final words".to_owned())) }
        },
    ));
    take.final_pipeline = Some(queue(commands, task, Arc::clone(&pending)));
    let held = saturate(&pending);
    take.append(closed_speech()).expect("speech PCM reserves");

    take.submit_closed_segments();

    assert!(take.pending_failure().is_none());
    assert_eq!(TakeState::lock(&take.state.buffer).origin(), 0);
    assert!(decodes.try_recv().is_err(), "nothing decodes while full");
    drop(held);
    take.submit_closed_segments();
    let decoded_samples = tokio::time::timeout(Duration::from_secs(5), decodes.recv())
        .await
        .expect("the retry decodes before the deadline")
        .expect("the pipeline decodes the held range");
    assert!(decoded_samples >= 16_000);
    let transcript = take
        .finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds");
    assert_eq!(transcript, "final words");
    assert!(
        decodes.try_recv().is_err(),
        "the silent tail is not decoded"
    );
}
