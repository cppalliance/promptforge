//! The bounded stdio transport.
//!
//! The convenience `rmcp::transport::stdio()` pairs raw stdin and stdout and
//! reads them with `read_until(b'\n', ...)` and no ceiling, so a spawning
//! harness that sends a line without a newline grows an in-memory buffer until
//! the process is out of memory. Local process ancestry is an authorization
//! assumption, not a memory bound, so this transport frames the same two
//! streams through a codec with a documented maximum line length: a line that
//! runs past the cap is dropped and the reader drains to the next newline
//! rather than buffering the whole thing, and the session goes on with the next
//! message.
//!
//! The framing is otherwise rmcp's own [`JsonRpcMessageCodec`], so BOM
//! stripping, CRLF handling, and the non-standard-notification compatibility
//! rules are exactly the ones the default transport applies. The read side
//! drives that codec directly rather than through a [`FramedRead`], because
//! `FramedRead` ends the stream on the first decoder error and would turn one
//! over-cap or malformed line into the end of the session; driving the decoder
//! by hand lets an over-cap line drain and the session go on.
//!
//! [`FramedRead`]: tokio_util::codec::FramedRead

use std::future::Future;
use std::sync::Arc;

use futures_util::SinkExt;
use rmcp::RoleServer;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::transport::async_rw::JsonRpcMessageCodec;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, FramedWrite};

/// The largest single JSON-RPC line the stdio reader will accept before it
/// drops the frame and drains to the next newline.
///
/// It matches the streamable-HTTP transport's inherited 4 MiB request-body
/// ceiling, so the two transports refuse an oversized message at the same size
/// rather than at two the operator has to learn separately. A well-formed
/// `tools/call` is orders of magnitude under it; a line approaching it is a
/// malformed or hostile peer, not a real request.
pub(crate) const MAX_STDIO_LINE_BYTES: usize = 4 * 1024 * 1024;

/// How much the read buffer grows before each read from the peer. The peak
/// buffer is one line's worth up to the cap plus one of these, so the read side
/// never holds more than the cap plus a chunk however the peer paces its bytes.
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// The decoder that turns capped byte lines into received messages.
type Codec = JsonRpcMessageCodec<RxJsonRpcMessage<RoleServer>>;

/// The framed writer half: rmcp's line codec, uncapped on the way out, over
/// `W`. A response this server writes is trusted; only the read side faces a
/// hostile peer.
type Writer<W> = FramedWrite<W, JsonRpcMessageCodec<TxJsonRpcMessage<RoleServer>>>;

/// A stdio transport whose read side caps the line length it will buffer.
///
/// Generic over its two byte streams so the process case (`stdin`, `stdout`)
/// and a test case (a pair of in-memory pipes) drive exactly the same framing.
pub(crate) struct BoundedStdioTransport<R, W> {
    read: R,
    buf: BytesMut,
    codec: Codec,
    write: Arc<Mutex<Option<Writer<W>>>>,
}

impl<R, W> BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    /// Frames `read` and `write` with the default line cap.
    pub(crate) fn new(read: R, write: W) -> BoundedStdioTransport<R, W> {
        BoundedStdioTransport::with_max_line(read, write, MAX_STDIO_LINE_BYTES)
    }

    /// Frames `read` and `write`, capping a read line at `max_line` bytes.
    ///
    /// The cap is a parameter so a test can prove the drop-and-drain behaviour
    /// against a small ceiling instead of having to synthesize four megabytes.
    pub(crate) fn with_max_line(read: R, write: W, max_line: usize) -> BoundedStdioTransport<R, W> {
        let write = FramedWrite::new(
            write,
            JsonRpcMessageCodec::<TxJsonRpcMessage<RoleServer>>::default(),
        );
        BoundedStdioTransport {
            read,
            buf: BytesMut::new(),
            codec: Codec::new_with_max_length(max_line),
            write: Arc::new(Mutex::new(Some(write))),
        }
    }
}

impl<R, W> Transport<RoleServer> for BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let lock = Arc::clone(&self.write);
        async move {
            let mut guard = lock.lock().await;
            match guard.as_mut() {
                Some(write) => write.send(item).await.map_err(std::io::Error::from),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "stdio transport is closed",
                )),
            }
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            // Decode whatever is already buffered first. An over-cap line makes
            // the codec error and start discarding to the next newline, and a
            // line that will not parse is consumed and skipped; either way the
            // frame is dropped, the buffer stays bounded, and the loop reads on
            // rather than ending the session. Only a real EOF or read error
            // stops it.
            match self.codec.decode(&mut self.buf) {
                Ok(Some(message)) => return Some(message),
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(
                        "stdio transport dropped an over-cap or malformed frame: {error}"
                    );
                    continue;
                }
            }
            self.buf.reserve(READ_CHUNK_BYTES);
            match self.read.read_buf(&mut self.buf).await {
                Ok(0) => {
                    return match self.codec.decode_eof(&mut self.buf) {
                        Ok(message) => message,
                        Err(error) => {
                            tracing::debug!("stdio transport dropped a trailing frame: {error}");
                            None
                        }
                    };
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!("stdio transport read failed: {error}");
                    return None;
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.write.lock().await.take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rmcp::transport::Transport;
    use serde_json::Value;

    use super::BoundedStdioTransport;

    /// One JSON-RPC line, newline-terminated, as bytes on the wire.
    fn line(json: &str) -> Vec<u8> {
        let mut bytes = json.as_bytes().to_vec();
        bytes.push(b'\n');
        bytes
    }

    /// The `method` of a received message, so a test reads what arrived without
    /// naming rmcp's message enum.
    fn method_of(message: &rmcp::service::RxJsonRpcMessage<rmcp::RoleServer>) -> String {
        serde_json::to_value(message)
            .expect("a received message serializes")
            .get("method")
            .and_then(Value::as_str)
            .expect("a request carries a method")
            .to_owned()
    }

    #[tokio::test]
    async fn a_normal_line_is_delivered() {
        let input = line(r#"{"jsonrpc":"2.0","method":"ping","id":1}"#);
        let mut transport =
            BoundedStdioTransport::with_max_line(&input[..], tokio::io::sink(), 1024);
        let message = transport.receive().await.expect("the ping is delivered");
        assert_eq!(method_of(&message), "ping");
    }

    #[tokio::test]
    async fn an_over_cap_line_is_dropped_and_the_next_message_survives() {
        // A line far past the cap, with no newline until its end, then a valid
        // ping. If the reader grew its buffer to the whole first line it would
        // have to hold 64 KiB against a 1 KiB cap; instead the codec caps the
        // buffer, errors, drains to the newline, and the ping after it is still
        // the next thing `receive` yields.
        let cap = 1024;
        let mut input = vec![b'x'; cap * 64];
        input.push(b'\n');
        input.extend_from_slice(&line(r#"{"jsonrpc":"2.0","method":"ping","id":1}"#));

        let mut transport =
            BoundedStdioTransport::with_max_line(&input[..], tokio::io::sink(), cap);
        let message = transport
            .receive()
            .await
            .expect("the ping after the over-cap line is delivered");
        assert_eq!(
            method_of(&message),
            "ping",
            "the over-cap line was drained, not buffered, and did not kill the session"
        );
    }

    #[tokio::test]
    async fn end_of_input_ends_the_session() {
        let input: &[u8] = b"";
        let mut transport = BoundedStdioTransport::with_max_line(input, tokio::io::sink(), 1024);
        assert!(
            transport.receive().await.is_none(),
            "a closed peer ends the stream rather than looping"
        );
    }
}
