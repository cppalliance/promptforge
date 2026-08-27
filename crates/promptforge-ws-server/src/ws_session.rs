//! Shared WebSocket session plumbing: the session-id counter, the outbound
//! message funnel, and the writer task that drains it into the socket.
//!
//! Every WebSocket endpoint has several tasks speaking to one client - the
//! receive loop plus a status forwarder or interim loop - so outbound
//! messages funnel through one channel into a single writer task that owns
//! the socket's sink half. [`WsSession`] owns that funnel and its teardown;
//! endpoint-specific tasks stay at their call sites.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::Message;
use futures_util::{Sink, SinkExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Session ids for log correlation, handed out in connection order across
/// every WebSocket endpoint.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Outbox depth: headroom for a streaming burst, while still holding a
/// producer back when the client stops reading.
const OUTBOX_CAPACITY: usize = 32;

/// One WebSocket session's outbound half: a session id for log
/// correlation, the outbox every task speaking to the client sends
/// through, and the writer task draining the outbox into the socket sink.
///
/// The writer runs until the outbox closes or a sink send fails (the
/// client is gone); [`WsSession::close`] tears both down.
#[derive(Debug)]
pub(crate) struct WsSession {
    id: u64,
    outbox: mpsc::Sender<Message>,
    writer: JoinHandle<()>,
}

impl WsSession {
    /// Claims the next session id and spawns the writer task that drains
    /// the outbox into `sink`.
    ///
    /// # Panics
    /// Panics when called outside a Tokio runtime, where the writer task
    /// cannot spawn.
    pub(crate) fn new<S>(mut sink: S) -> Self
    where
        S: Sink<Message> + Send + Unpin + 'static,
    {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let (outbox, mut out_rx) = mpsc::channel::<Message>(OUTBOX_CAPACITY);
        let writer = tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        Self { id, outbox, writer }
    }

    /// The session's id, for log correlation.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The outbox every task speaking to this client sends through.
    pub(crate) fn outbox(&self) -> &mpsc::Sender<Message> {
        &self.outbox
    }

    /// Tears the session down: drops the session's own outbox handle and
    /// aborts the writer, so nothing more reaches the client even through
    /// outstanding outbox clones.
    pub(crate) fn close(self) {
        drop(self.outbox);
        self.writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    /// A stub client socket: every message the writer sends lands on the
    /// paired receiver, and dropping the receiver fails the next send the
    /// way a closed socket would.
    struct StubSink {
        forward: mpsc::UnboundedSender<Message>,
    }

    fn stub_sink() -> (StubSink, mpsc::UnboundedReceiver<Message>) {
        let (forward, received) = mpsc::unbounded_channel();
        (StubSink { forward }, received)
    }

    impl Sink<Message> for StubSink {
        type Error = mpsc::error::SendError<Message>;

        fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.forward.send(item)
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Reads one text message from the stub receiver, failing on anything
    /// else.
    async fn read_text(received: &mut mpsc::UnboundedReceiver<Message>) -> String {
        match received.recv().await {
            Some(Message::Text(text)) => text.to_string(),
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_ids_increase_in_creation_order() {
        let (first_sink, _first_rx) = stub_sink();
        let (second_sink, _second_rx) = stub_sink();
        let first = WsSession::new(first_sink);
        let second = WsSession::new(second_sink);
        // Other tests in this binary bump the shared counter concurrently,
        // so the ids need not be adjacent - only ordered.
        assert!(
            second.id() > first.id(),
            "ids are handed out in connection order: {} then {}",
            first.id(),
            second.id()
        );
        first.close();
        second.close();
    }

    #[tokio::test]
    async fn outbox_messages_reach_the_sink_through_the_writer() {
        let (sink, mut received) = stub_sink();
        let session = WsSession::new(sink);
        for text in ["one", "two"] {
            session
                .outbox()
                .send(Message::Text(text.into()))
                .await
                .expect("the writer holds the outbox open");
        }
        assert_eq!(read_text(&mut received).await, "one");
        assert_eq!(read_text(&mut received).await, "two", "order is preserved");
        session.close();
    }

    #[tokio::test]
    async fn a_failed_sink_send_stops_the_writer_and_closes_the_outbox() {
        let (sink, received) = stub_sink();
        let session = WsSession::new(sink);
        // A dropped receiver fails the writer's next sink send, standing in
        // for a client that closed the socket.
        drop(received);
        let _ = session.outbox().send(Message::Text("lost".into())).await;
        tokio::time::timeout(Duration::from_secs(5), session.outbox().closed())
            .await
            .expect("the writer stops on the failed send, closing the outbox");
        assert!(session.outbox().is_closed());
        session.close();
    }

    #[tokio::test]
    async fn close_aborts_the_writer_and_closes_the_outbox() {
        let (sink, mut received) = stub_sink();
        let session = WsSession::new(sink);
        let outbox = session.outbox().clone();
        session.close();
        // The aborted writer drops its outbox receiver, so even an
        // outstanding clone observes the channel closing.
        tokio::time::timeout(Duration::from_secs(5), outbox.closed())
            .await
            .expect("close aborts the writer, closing the outbox");
        assert!(
            received.recv().await.is_none(),
            "the writer is gone, so nothing more reaches the sink"
        );
    }
}
