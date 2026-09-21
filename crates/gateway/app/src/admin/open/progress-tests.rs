//! The progress SSE stream: snapshot first, one line per change,
//! heartbeats, and shutdown.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use gateway_api_types::Progress;
use gateway_progress::ProgressHub;

use super::{PROGRESS_HEARTBEAT, progress_sse_response};
use crate::shutdown::ShutdownSignal;

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

fn snapshot(busy: bool, text: &str) -> Progress {
    Progress {
        busy,
        text: text.to_owned(),
    }
}

/// Reads `data:` payloads from the body until `count` snapshots arrive,
/// skipping heartbeat comments. Frames may split or coalesce SSE events,
/// so the text accumulates across reads.
async fn read_snapshots<S>(frames: &mut S, count: usize) -> Vec<Progress>
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    let mut text = String::new();
    let mut snapshots = Vec::new();
    while snapshots.len() < count {
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
                snapshots.push(serde_json::from_str(data).expect("a data line is a Progress"));
            }
        }
    }
    snapshots
}

#[tokio::test]
async fn a_fresh_subscriber_first_receives_the_current_snapshot() {
    let hub = Arc::new(ProgressHub::new());
    let activity = hub.begin("Downloading qwen 45%");

    // The subscriber connects after the work began: the stream must open
    // with the current state rather than wait for the next change.
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let snapshots = read_snapshots(&mut frames, 1).await;
    assert_eq!(snapshots[0], snapshot(true, "Downloading qwen 45%"));
    drop(activity);
}

#[tokio::test]
async fn an_idle_hub_opens_the_stream_with_the_idle_snapshot() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let snapshots = read_snapshots(&mut frames, 1).await;
    assert_eq!(
        snapshots[0],
        Progress::default(),
        "the idle snapshot is sent so a subscriber can clear a stale bar at once"
    );
}

#[tokio::test]
async fn the_stream_sends_one_line_per_change_in_order() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    // Consume the opening idle snapshot.
    let opening = read_snapshots(&mut frames, 1).await;
    assert_eq!(opening[0], Progress::default());

    let activity = hub.begin("Loading profile");
    let begun = read_snapshots(&mut frames, 1).await;
    assert_eq!(begun[0], snapshot(true, "Loading profile"));

    activity.set_text("Downloading models");
    let moved = read_snapshots(&mut frames, 1).await;
    assert_eq!(moved[0], snapshot(true, "Downloading models"));

    drop(activity);
    let ended = read_snapshots(&mut frames, 1).await;
    assert_eq!(
        ended[0],
        Progress::default(),
        "the last activity's drop publishes the idle snapshot"
    );
}

#[tokio::test(start_paused = true)]
async fn an_idle_hub_emits_heartbeat_comments_on_cadence() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let opening = read_snapshots(&mut frames, 1).await;
    assert_eq!(opening[0], Progress::default());

    // Nothing is live, so heartbeat comments are the only traffic; two
    // ticks pin the cadence, not just the first deadline.
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
async fn an_activity_drop_reports_idle_then_the_stream_goes_quiet() {
    let hub = Arc::new(ProgressHub::new());
    let response = progress_sse_response(&hub, ShutdownSignal::default());
    let mut frames = response.into_body().into_data_stream();
    let opening = read_snapshots(&mut frames, 1).await;
    assert_eq!(opening[0], Progress::default());

    let activity = hub.begin("Downloading qwen");
    let begun = read_snapshots(&mut frames, 1).await;
    assert!(begun[0].busy);

    drop(activity);
    let ended = read_snapshots(&mut frames, 1).await;
    assert!(!ended[0].busy);
    // The first heartbeat is 15 s out, so nothing may arrive inside this
    // window after the drop: an idle hub is otherwise silent.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), frames.next())
            .await
            .is_err(),
        "a dropped activity must leave the stream quiet until the next heartbeat"
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

    // The activity begins before the first poll, so the opening line is
    // already the busy snapshot: the watch keeps only the latest state.
    let _activity = hub.begin("Downloading qwen");
    let snapshots = read_snapshots(&mut frames, 1).await;
    assert_eq!(snapshots[0], snapshot(true, "Downloading qwen"));

    shutdown.fire();
    let end = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
        .await
        .expect("the stream reacts to the signal within the frame timeout");
    assert!(
        end.is_none(),
        "the stream ends on shutdown instead of waiting for the heartbeat: {end:?}"
    );
}
