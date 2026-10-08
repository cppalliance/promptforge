//! Integration tests for realtime session lifecycle, commits, and cancellation.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use gateway_stt::test_fixtures::{
    FixtureError, RealtimeSessionFixture, RealtimeSessionRegistryFixture,
};

mod commit;
mod end_session;
mod finalization;
mod interim;
mod lifecycle;

const SESSION_CAPACITY: usize = 8;
const CANCEL_JOIN_CAPACITY: usize = 8;
const COMMITTED_ITEM_CAPACITY: usize = 4;
static BLOCKING_TASK_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn encoded(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn closed_segment() -> String {
    let mut samples = vec![16_384; 24_000];
    samples.extend(vec![0; 72_000]);
    encoded(&samples)
}

fn update(prompt: &str, include: bool) -> String {
    serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {"input": {"transcription": {"prompt": prompt}}},
            "include": if include {
                vec!["item.input_audio_transcription.hypothesis"]
            } else {
                Vec::<&str>::new()
            }
        }
    })
    .to_string()
}

#[expect(
    clippy::expect_used,
    reason = "a fixture registry has no prior session that could consume capacity"
)]
fn session() -> RealtimeSessionFixture {
    RealtimeSessionRegistryFixture::default()
        .register()
        .expect("session registers")
}

fn source_message(error: &FixtureError) -> Option<String> {
    std::error::Error::source(error).map(ToString::to_string)
}

/// A shared buffer that collects what a capture subscriber writes.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    fn lines_containing(&self, needle: &str) -> Vec<String> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| line.contains(needle))
            .map(str::to_owned)
            .collect()
    }
}

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs a DEBUG-level subscriber writing to a fresh capture buffer for
/// the current thread. Only a current-thread test runtime keeps every poll
/// on this thread, so the session's events land in the buffer.
fn capture_debug_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (buffer, guard)
}

struct BlockingPoll {
    started: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<AtomicBool>,
}

impl Future for BlockingPoll {
    type Output = String;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut started = self
            .started
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *started = true;
        self.started.1.notify_all();
        drop(started);
        while !self.release.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        Poll::Ready("released".to_owned())
    }
}

struct BlockingFinalization(BlockingPoll);

impl Future for BlockingFinalization {
    type Output = anyhow::Result<String>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(context).map(Ok)
    }
}

fn wait_until_started(started: &Arc<(Mutex<bool>, Condvar)>) {
    let state = started
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (state, timeout) = started
        .1
        .wait_timeout_while(state, Duration::from_secs(1), |started| !*started)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(!timeout.timed_out() && *state, "blocked task starts");
}

async fn wait_until(predicate: impl Fn() -> bool) {
    assert!(
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if predicate() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok(),
        "condition reaches its wall-clock deadline"
    );
}

#[expect(
    clippy::expect_used,
    reason = "the helper establishes valid canonical fixture audio and input"
)]
fn append_committable(session: &mut RealtimeSessionFixture) -> String {
    session
        .append_base64(&encoded(&vec![0; 2_400]))
        .expect("committable input appends");
    session
        .input_snapshot()
        .expect("provisional input exists")
        .item_id()
        .to_owned()
}

#[expect(
    clippy::expect_used,
    reason = "the helper establishes valid decodable fixture audio"
)]
fn append_decodable(session: &mut RealtimeSessionFixture) {
    session
        .append_base64(&encoded(&vec![512; 12_000]))
        .expect("decodable input appends");
}
