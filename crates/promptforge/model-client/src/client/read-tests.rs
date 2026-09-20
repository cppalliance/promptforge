//! Tests for capped body reads and SSE completion-stream reassembly.

use std::cell::Cell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::client::CompletionResult;
use crate::model::CompletionErrorKind;

/// A chunk source over canned chunks; it never pends, so the tests need
/// no executor.
struct Canned(VecDeque<Result<Vec<u8>, CompletionError>>);

impl Canned {
    fn of(chunks: &[&str]) -> Canned {
        Canned(
            chunks
                .iter()
                .map(|chunk| Ok(chunk.as_bytes().to_vec()))
                .collect(),
        )
    }
}

impl ChunkSource for Canned {
    type Chunk = Vec<u8>;

    fn next_chunk(
        &mut self,
    ) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send {
        std::future::ready(self.0.pop_front().transpose())
    }
}

/// Drives a future that never pends to its output.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

fn sse(chunks: &[serde_json::Value]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(&chunk.to_string());
        body.push_str("\n\n");
    }
    body
}

fn text_chunk(text: &str) -> serde_json::Value {
    json!({ "choices": [{ "index": 0, "delta": { "content": text } }] })
}

#[test]
fn read_body_capped_refuses_an_advertised_oversize_length_before_reading() {
    let mut source = Canned::of(&["never read"]);
    let err = block_on(read_body_capped(&mut source, Some(100), 8))
        .expect_err("an advertised length over the cap is refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(err.to_string().contains("100 bytes"), "got {err}");
    assert_eq!(source.0.len(), 1, "nothing was read");
}

#[test]
fn read_body_capped_refuses_streamed_chunks_over_the_cap() {
    let mut source = Canned::of(&["12345", "6789"]);
    let err = block_on(read_body_capped(&mut source, None, 8))
        .expect_err("chunks past the cap are refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(err.to_string().contains("8-byte"), "got {err}");
}

#[test]
fn read_body_capped_returns_a_body_within_the_cap() {
    let mut source = Canned::of(&["12345", "678"]);
    let body = block_on(read_body_capped(&mut source, Some(8), 8)).expect("within the cap");
    assert_eq!(body, b"12345678");
}

#[test]
fn read_completion_stream_reassembles_the_turn_and_times_it_on_the_injected_clock() {
    let body = sse(&[
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant" } }] }),
        text_chunk("hel"),
        text_chunk("lo"),
        json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
        json!("[DONE]"),
    ])
    .replace("data: \"[DONE]\"", "data: [DONE]");
    let (head, tail) = body.split_at(body.len() / 2);
    let mut source = Canned::of(&[head, tail]);
    let started = Instant::now();
    // The clock advances 10ms per reading: delta one at +10, delta two at
    // +20, the end-to-end reading at +30. Nothing here reads a real clock.
    let ticks = Cell::new(0_u32);
    let now = || {
        ticks.set(ticks.get() + 1);
        started + Duration::from_millis(10 * u64::from(ticks.get()))
    };
    let seen = std::cell::RefCell::new(Vec::new());
    let completion = block_on(read_completion_stream(
        &mut source,
        json!({ "model": "m" }),
        1024,
        |delta| seen.borrow_mut().push(delta),
        started,
        now,
    ))
    .expect("a whole stream reassembles");
    assert_eq!(
        completion.result(),
        &CompletionResult::Text("hello".to_owned())
    );
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(seen.borrow().len(), 2, "one live delta per text fragment");
    let timing = completion.client_timing().expect("timing is measured");
    assert!((timing.ttft_ms.expect("first delta") - 10.0).abs() < f64::EPSILON);
    assert!((timing.mean_itl_ms.expect("two deltas") - 10.0).abs() < f64::EPSILON);
    assert!((timing.e2e_ms - 30.0).abs() < f64::EPSILON);
}

#[test]
fn read_completion_stream_refuses_a_stream_over_the_byte_cap() {
    let body = sse(&[text_chunk("a long reply")]);
    let mut source = Canned::of(&[&body]);
    let started = Instant::now();
    let err = block_on(read_completion_stream(
        &mut source,
        json!({}),
        8,
        |_| {},
        started,
        || started,
    ))
    .expect_err("an oversize stream is refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(err.to_string().contains("8-byte"), "got {err}");
}

#[test]
fn read_completion_stream_refuses_a_stream_without_the_sentinel() {
    let body = sse(&[text_chunk("half")]);
    let mut source = Canned::of(&[&body]);
    let started = Instant::now();
    let err = block_on(read_completion_stream(
        &mut source,
        json!({}),
        1024,
        |_| {},
        started,
        || started,
    ))
    .expect_err("a cut-off stream is refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(err.to_string().contains("[DONE]"), "got {err}");
}

#[test]
fn read_completion_stream_returns_the_source_failure_as_is() {
    let timed_out = CompletionError::from(Error::http(crate::Timeout(Box::new(
        std::io::Error::new(std::io::ErrorKind::TimedOut, "deadline"),
    ))));
    let mut source = Canned(VecDeque::from([
        Ok(sse(&[text_chunk("par")]).into_bytes()),
        Err(timed_out),
    ]));
    let started = Instant::now();
    let err = block_on(read_completion_stream(
        &mut source,
        json!({}),
        1024,
        |_| {},
        started,
        || started,
    ))
    .expect_err("a read failure fails the round");
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    assert!(err.is_timeout(), "the marker survives: {err:?}");
}
