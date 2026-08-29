//! The per-chat tape guard of the direct-gateway chat adapter: the tape
//! bookkeeping carried through one streaming chat, doubling as the
//! disconnect guard. Distinct from the crate-root [`crate::tape`], the
//! durable session tape this guard writes into.

use std::sync::Arc;
use std::time::Instant;

use crate::push::Push;
use crate::relay::tape_round_trip;
use crate::tape::Tape;

/// Tape bookkeeping carried through one streaming chat, doubling as the
/// disconnect guard.
///
/// The settle paths consume it through [`StreamTape::record`], so a
/// streamed chat always tapes exactly one event; a chat abandoned
/// mid-stream drops it un-recorded, and the drop spawns the tape write
/// with the disconnect note and then returns the status bar to Ready -
/// the session loop's exit paths carry no cleanup calls.
pub(super) struct StreamTape {
    /// Present until the chat settles; taken exactly once, by `record` or
    /// by the drop.
    entry: Option<TapeEntry>,
    push: Push,
}

/// What one tape event needs from the chat that produced it.
struct TapeEntry {
    tape: Arc<Tape>,
    model: String,
    request: serde_json::Value,
    started: Instant,
    /// Concatenation of every content delta forwarded so far.
    assembled: String,
}

impl StreamTape {
    /// Arms the guard for one streaming chat.
    pub(super) fn open(
        tape: Arc<Tape>,
        model: String,
        request: serde_json::Value,
        started: Instant,
        push: Push,
    ) -> Self {
        Self {
            entry: Some(TapeEntry {
                tape,
                model,
                request,
                started,
                assembled: String::new(),
            }),
            push,
        }
    }

    /// Appends one forwarded content delta to the assembled response.
    pub(super) fn append(&mut self, text: &str) {
        if let Some(entry) = self.entry.as_mut() {
            entry.assembled.push_str(text);
        }
    }

    /// Writes the stream's single tape event: the assembled content on
    /// success, or `error` beside the partial content on failure.
    pub(super) async fn record(mut self, error: Option<String>) {
        if let Some(entry) = self.entry.take() {
            entry.write(error).await;
        }
    }

    /// Writes the declined-stream tape event: the gateway's buffered
    /// error envelope verbatim, in place of an assembled response.
    pub(super) async fn record_envelope(mut self, response: serde_json::Value) {
        if let Some(entry) = self.entry.take() {
            let TapeEntry {
                tape,
                model,
                request,
                started,
                ..
            } = entry;
            tape_round_trip(&tape, model, request, response, started.elapsed()).await;
        }
    }

    /// Disarms the guard without taping: the open failed before any
    /// response arrived, so no exchange happened and nothing is taped -
    /// matching the heartbeat short-circuit's no-tape rule.
    pub(super) fn discard(mut self) {
        self.entry = None;
    }
}

impl Drop for StreamTape {
    fn drop(&mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        let push = self.push.clone();
        // Drop cannot await, so the abandoned exchange is taped from a
        // spawned task; the idle push follows the write inside that task,
        // so a status observer that sees Ready can trust the tape to hold
        // the disconnect note.
        tokio::spawn(async move {
            entry
                .write(Some("client disconnected mid-stream".to_string()))
                .await;
            push.push_idle();
        });
    }
}

impl TapeEntry {
    /// Writes the tape event this entry was collected for.
    async fn write(self, error: Option<String>) {
        let Self {
            tape,
            model,
            request,
            started,
            assembled,
        } = self;
        let response = match error {
            Some(message) => serde_json::json!({
                "error": message,
                "content": assembled,
            }),
            None => serde_json::Value::String(assembled),
        };
        tape_round_trip(&tape, model, request, response, started.elapsed()).await;
    }
}
