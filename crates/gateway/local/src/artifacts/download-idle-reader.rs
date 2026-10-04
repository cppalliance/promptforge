//! The idle-bounded body reader: a thread forwards response chunks under a read deadline.

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use reqwest::blocking::Response;

/// Streams a blocking response body through a channel so the download loop
/// receives each chunk under an idle deadline. The blocking reqwest client
/// exposes no per-read timeout, so the reader thread owns the
/// response and forwards every read; a peer that goes silent past the
/// deadline surfaces as [`std::io::ErrorKind::TimedOut`] at the chunk
/// boundary where the cancellation token is already checked, and the staged
/// partial stays resumable. A read parked past the deadline stays parked
/// until the client's whole-request ceiling drops the body, which ends the
/// thread - the wait is bounded and the thread always reaps.
pub(super) struct IdleReader {
    chunks: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    idle: Duration,
}

impl IdleReader {
    /// Spawns the reader thread draining `response`.
    ///
    /// # Errors
    /// Returns the thread-spawn failure.
    pub(super) fn new(mut response: Response, idle: Duration) -> std::io::Result<IdleReader> {
        let (sender, chunks) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("artifact-download-reader".to_owned())
            .spawn(move || {
                let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
                loop {
                    let chunk = response
                        .read(&mut buffer)
                        .map(|count| buffer[..count].to_vec());
                    // An empty chunk is the end of the stream; after an error
                    // or EOF there is nothing more to send.
                    let terminal =
                        chunk.is_err() || matches!(&chunk, Ok(bytes) if bytes.is_empty());
                    if sender.send(chunk).is_err() || terminal {
                        break;
                    }
                }
            })?;
        Ok(IdleReader { chunks, idle })
    }

    /// The next body chunk; an empty chunk is the end of the stream.
    ///
    /// # Errors
    /// Returns the read error the thread forwarded, or
    /// [`std::io::ErrorKind::TimedOut`] when no chunk arrived inside the
    /// idle window.
    pub(super) fn read_chunk(&self) -> std::io::Result<Vec<u8>> {
        match self.chunks.recv_timeout(self.idle) {
            Ok(chunk) => chunk,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "the peer sent nothing for {:?}; the transfer stalled",
                    self.idle
                ),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the download reader thread stopped without reporting",
            )),
        }
    }
}
