//! The progress SSE stream: replay, heartbeats, lag, and shutdown.

// Fractions are fixed-point millionths, so equality comparisons are exact.
#![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use shared_progress::{EventState, ProgressEvent, ProgressHub};

use super::{PROGRESS_HEARTBEAT, progress_sse_response};
use crate::shutdown::ShutdownSignal;

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads `data:` payloads from the body until `count` events arrive,
/// skipping heartbeat comments. Frames may split or coalesce SSE events,
/// so the text accumulates across reads.
async fn read_events<S>(frames: &mut S, count: usize) -> Vec<ProgressEvent>
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    let mut text = String::new();
    let mut events = Vec::new();
    while events.len() < count {
        let frame = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
            .await
            .expect("the progress stream stalled")
            .expect("the progress stream ended early")
            .expect("the progress stream errored");
        let chunk = std::str::from_utf8(&frame).expect("SSE frames are UTF-8");
        text.push_str(chunk);
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            if let Some(data) = block.trim().strip_prefix("data: ") {
                events.push(serde_json::from_str(data).expect("a data line is a ProgressEvent"));
            }
        }
    }
    events
}

/// Reads `data:` payloads until `stop` matches one, returning everything
/// read, the matching event last.
async fn read_until<S>(frames: &mut S, stop: impl Fn(&ProgressEvent) -> bool) -> Vec<ProgressEvent>
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    let mut text = String::new();
    let mut events = Vec::new();
    loop {
        let frame = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
            .await
            .expect("the progress stream stalled")
            .expect("the progress stream ended early")
            .expect("the progress stream errored");
        let chunk = std::str::from_utf8(&frame).expect("SSE frames are UTF-8");
        text.push_str(chunk);
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            if let Some(data) = block.trim().strip_prefix("data: ") {
                let event: ProgressEvent =
                    serde_json::from_str(data).expect("a data line is a ProgressEvent");
                let done = stop(&event);
                events.push(event);
                if done {
                    return events;
                }
            }
        }
    }
}

#[tokio::test]
async fn the_stream_carries_begun_updated_finished_in_order() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();

    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    leaf.complete();

    let events = read_events(&mut frames, 3).await;
    assert!(matches!(events[0].state, EventState::Begun { weight } if weight == 1.0));
    assert!(
        matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5),
        "the intermediate sample follows Begun: {:?}",
        events[1]
    );
    assert!(matches!(events[2].state, EventState::Finished { ok: true }));
    assert!(events.iter().all(|event| event.path == "download"));
}

#[tokio::test]
async fn a_fresh_subscriber_first_receives_a_snapshot_of_live_operations() {
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);

    // The subscriber connects after the work began, so the broadcast
    // alone would show nothing until the next report: the snapshot must
    // carry the current state.
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let events = read_events(&mut frames, 2).await;
    assert!(matches!(events[0].state, EventState::Begun { weight } if weight == 1.0));
    assert!(matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5));
    assert_eq!(events[0].operation, tree.operation());
}

#[tokio::test]
async fn a_fresh_subscriber_sees_a_finished_leafs_terminal_state() {
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    leaf.fail();

    // The leaf finished before the subscriber connected; without a
    // replayed Finished the subscriber would hold it as unfinished until
    // the tree detaches.
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let events = read_events(&mut frames, 3).await;
    assert!(matches!(events[0].state, EventState::Begun { .. }));
    assert!(
        matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5),
        "a failed leaf keeps its fraction: {:?}",
        events[1]
    );
    assert!(
        matches!(events[2].state, EventState::Finished { ok: false }),
        "the terminal state replays: {:?}",
        events[2]
    );
}

#[tokio::test]
async fn a_subscriber_sees_when_the_complete_operation_detaches() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let tree = hub.operation();
    let operation = tree.operation();
    let leaf = tree.register("loading-profile", 1.0);
    leaf.complete();
    drop(tree);

    let events = read_until(&mut frames, |event| {
        matches!(event.state, EventState::OperationFinished)
    })
    .await;
    assert_eq!(
        events.last().map(|event| event.operation),
        Some(operation),
        "the terminal lifecycle event names the detached operation"
    );
}

#[tokio::test]
async fn a_lagged_subscriber_drops_the_overflow_and_carries_on() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();

    let tree = hub.operation();
    // Overflow the hub's 1024-event broadcast ring before the stream's
    // first poll, so its receiver lags: the Lagged arm must drop the
    // skipped events and continue rather than ending the stream.
    let _leaves: Vec<_> = (0..1100)
        .map(|index| tree.register(&format!("leaf-{index}"), 1.0))
        .collect();
    let last = tree.register("last", 1.0);
    last.complete();

    let events = read_until(&mut frames, |event| {
        event.path == "last" && matches!(event.state, EventState::Finished { ok: true })
    })
    .await;
    assert!(
        events.len() <= 1024,
        "the overflowed prefix is dropped, not delivered: {} events",
        events.len()
    );
}

#[tokio::test(start_paused = true)]
async fn an_idle_hub_emits_heartbeat_comments_on_cadence() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();

    // No operations are live, so heartbeat comments are the only
    // traffic; two ticks pin the cadence, not just the first deadline.
    for _ in 0..2 {
        let frame = tokio::time::timeout(PROGRESS_HEARTBEAT + FRAME_TIMEOUT, frames.next())
            .await
            .expect("the progress stream stalled")
            .expect("the progress stream ended early")
            .expect("the progress stream errored");
        assert_eq!(
            std::str::from_utf8(&frame).expect("SSE frames are UTF-8"),
            ": heartbeat\n\n"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_tree_drop_reports_completion_then_the_stream_goes_quiet() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();

    let tree = hub.operation();
    let operation = tree.operation();
    let _leaf = tree.register("download", 1.0);
    let events = read_events(&mut frames, 1).await;
    assert!(matches!(events[0].state, EventState::Begun { .. }));

    drop(tree);
    let events = read_events(&mut frames, 1).await;
    assert_eq!(events[0].operation, operation);
    assert!(matches!(events[0].state, EventState::OperationFinished));
    // The first heartbeat is 15 s out, so nothing may arrive inside this
    // window after completion: an idle hub is otherwise silent.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), frames.next())
            .await
            .is_err(),
        "a dropped tree must leave the stream quiet until the next heartbeat"
    );
}

/// The shutdown signal ends the open-ended stream, so an attached
/// subscriber cannot hold its connection through the graceful drain.
#[tokio::test]
async fn the_stream_ends_when_the_shutdown_signal_fires() {
    let hub = Arc::new(ProgressHub::new());
    let shutdown = ShutdownSignal::default();
    let response = progress_sse_response(&hub, shutdown.clone());
    let mut frames = response.into_body().into_data_stream();

    let tree = hub.operation();
    let _leaf = tree.register("download", 1.0);
    let events = read_events(&mut frames, 1).await;
    assert!(matches!(events[0].state, EventState::Begun { .. }));

    shutdown.fire();
    let end = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
        .await
        .expect("the stream reacts to the signal within the frame timeout");
    assert!(
        end.is_none(),
        "the stream ends on shutdown instead of waiting for the heartbeat: {end:?}"
    );
}
