use futures_util::StreamExt as _;
use gateway_progress::ProgressHub;

use super::*;

/// Serves `body` at `/model.bin` with an accurate Content-Length and
/// returns its URL.
async fn fake_file_server(body: &'static [u8]) -> String {
    let app = axum::Router::new().route(
        "/model.bin",
        axum::routing::get(move || async move {
            axum::response::Response::new(axum::body::Body::from(body))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the fake server binds");
    let address = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("the fake server serves");
    });
    format!("http://{address}/model.bin")
}

/// Collects a response body's full text.
async fn body_text(response: Response) -> String {
    let mut frames = response.into_body().into_data_stream();
    let mut text = String::new();
    while let Some(frame) = frames.next().await {
        let frame = frame.expect("the stream errored");
        text.push_str(std::str::from_utf8(&frame).expect("SSE frames are UTF-8"));
    }
    text
}

#[tokio::test]
async fn a_download_drives_the_hub_text_and_ends_its_activity_at_completion() {
    let body = b"activity-visibility-fixture";
    let url = fake_file_server(body).await;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().to_path_buf();
    let hub = Arc::new(ProgressHub::new());

    let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
    assert_eq!(
        hub.current(),
        gateway_api_types::Progress {
            busy: true,
            text: "Downloading model.bin".to_owned(),
        },
        "the reporter names the download before any byte arrives"
    );

    // The blocking reqwest client inside `BlobCache` cannot be built or
    // dropped in async context, so the whole store lifecycle runs on the
    // blocking pool, as it does in the route.
    let reporter = Arc::clone(&progress);
    tokio::task::spawn_blocking(move || {
        let cache = BlobCache::new(root).expect("the cache opens");
        cache.download_to_cache(&url, None, reporter.as_ref())
    })
    .await
    .expect("the download task joins")
    .expect("the download succeeds");
    assert_eq!(
        hub.current().text,
        "Downloading model.bin 100%",
        "the last byte drives the text to its final whole percent"
    );
    assert_eq!(
        progress.sample(),
        (body.len() as u64, Some(body.len() as u64)),
        "the byte counts stay alongside the text for the SSE payload"
    );

    drop(progress);
    assert!(
        !hub.current().busy,
        "the reporter's drop ends the download's activity"
    );
}

#[tokio::test]
async fn the_sse_stream_derives_from_byte_samples_and_ends_with_the_join_result() {
    let body = b"sse-sample-derived-fixture";
    let url = fake_file_server(body).await;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().to_path_buf();
    let hub = Arc::new(ProgressHub::new());

    let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
    let rx = progress.subscribe();
    let join = tokio::task::spawn_blocking(move || {
        let cache = BlobCache::new(root).expect("the cache opens");
        let result = cache.download_to_cache(&url, None, progress.as_ref());
        drop(progress);
        result
    });

    let text = body_text(sse_response(rx, join)).await;
    let events: Vec<serde_json::Value> = text
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .map(|block| {
            let data = block.trim().strip_prefix("data: ").expect("a data line");
            serde_json::from_str(data).expect("a data line is JSON")
        })
        .collect();
    let (terminal, progress) = events.split_last().expect("the stream has events");
    assert_eq!(terminal["status"], "ready");
    let path = std::path::PathBuf::from(terminal["path"].as_str().expect("path"));
    assert_eq!(
        std::fs::read(&path).expect("read blob"),
        body,
        "the terminal event names the cached blob"
    );
    assert!(
        !progress.is_empty(),
        "progress events precede the terminal event: {text}"
    );
    let last = progress.last().expect("a progress event");
    assert_eq!(last["status"], "downloading");
    assert_eq!(
        last["bytes"],
        body.len() as u64,
        "the final sample reports every byte: {text}"
    );
    assert_eq!(last["total"], body.len() as u64);
    assert!(
        !hub.current().busy,
        "the finished download released its activity"
    );
}

#[tokio::test]
async fn the_sse_stream_emits_the_latest_sample_when_the_reporter_dropped_before_the_first_poll() {
    // The download completes (and the reporter drops) before the
    // stream is polled at all: the watch's closed channel must not
    // hide the final byte sample, which precedes the terminal event.
    let body = b"sse-late-poll-fixture";
    let url = fake_file_server(body).await;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().to_path_buf();
    let hub = Arc::new(ProgressHub::new());

    let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
    let rx = progress.subscribe();
    let result = tokio::task::spawn_blocking(move || {
        let cache = BlobCache::new(root).expect("the cache opens");
        let result = cache.download_to_cache(&url, None, progress.as_ref());
        drop(progress);
        result
    })
    .await
    .expect("the download task joins");
    let join = tokio::task::spawn_blocking(move || result);

    let text = body_text(sse_response(rx, join)).await;
    let blocks: Vec<&str> = text
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .collect();
    assert_eq!(
        blocks.len(),
        2,
        "one final sample, then the terminal: {text}"
    );
    assert!(
        blocks[0].contains("\"status\":\"downloading\"")
            && blocks[0].contains(&format!("\"bytes\":{}", body.len())),
        "the latest sample is emitted even though the channel closed: {text}"
    );
    assert!(
        blocks[1].contains("\"status\":\"ready\""),
        "the stream still ends with the join result's terminal event: {text}"
    );
}
