//! Progress for a run in flight: the observer that queues it, the pump that
//! sends it.
//!
//! One `tools/call` can take minutes, and `notifications/progress` is the only
//! thing a client can render while it waits. The two halves of that job have
//! incompatible shapes: the core reports a run through [`Observer`], whose
//! `observe` is synchronous and sits on the run's own path, while sending a
//! notification is an `.await` on the peer. [`McpObserver`] is the join between
//! them. `observe` recognizes a pinned detail vocabulary, turns the two
//! cosmetic progress details into frames, and hands them to a bounded
//! channel with `try_send`, and [`ProgressPump`] awaits the peer on a task of
//! its own. Nothing on the run's path blocks, and the only shared state is a
//! set of atomics, so no guard exists to be held across an await point.
//!
//! A frame that finds the queue full is dropped and counted. Progress is a
//! report and never a decision, so a client reading its stream slowly must cost
//! itself frames rather than cost the run its latency; the count is what leaves
//! the loss in a log instead of nowhere. The final flush is bounded for the
//! same reason: [`ProgressPump::finish`] waits briefly for the last frames and
//! then abandons them, since a caption nobody reads is not worth a reply
//! nobody receives.
//!
//! What each recognized observation produces:
//!
//! | Detail | Frame |
//! |---|---|
//! | `Run started` | frame 0, captioned with the H1 title |
//! | `Section started` | the incremented section count, captioned with the H2 |
//! | all other recognized and unknown details | none; logged only |
//!
//! `Section finished` sends nothing because the frame it would send is the one
//! already on the wire: same latched `progress`, same section name. A client
//! renders the caption in place, so a second identical frame is invisible to
//! the reader and pure cost on the stream.
//!
//! What each observation logs, which is a separate question from what it sends:
//! the start boundary is `info`. The runner logs the terminal [`RunResult`]
//! separately at `info`, once status, elapsed time, and the final turn count
//! exist. Everything inside a run is `debug`, because a section can make dozens
//! of tool calls and burying the boundaries under them defeats the purpose. The
//! exceptions are the two within-run failures - a failed tool call and a failed
//! model turn - which are `warn`: each is a thing an operator wants without
//! turning on debug, and both are rare enough not to flood.
//!
//! No log line carries author-controlled content. The `execution` name, the
//! `section` name, and an unrecognized detail's payload are all untrusted per
//! the [`Observer`] contract, so the log records only stable counts and fixed
//! messages; a prompt cannot write the server's trusted log stream. The
//! `section` name still reaches the client, but on the progress channel that is
//! its purpose, not the operator log.
//!
//! [`RunResult`]: crate::result::RunResult
//!
//! `total` is never sent. A `jump` or an early return means the number of
//! sections a run will visit is unknown when it starts, so a denominator would
//! be a guess wearing a measurement's clothes; the client shows a changing
//! caption rather than a filling bar. `progress` is latched with a maximum, so
//! it never decreases, which the protocol requires.

#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use promptforge_core::observe::{Observation, Observer};
use rmcp::RoleServer;
use rmcp::model::ProgressToken;
use rmcp::service::Peer;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Receiver, Sender};

mod pump;
#[cfg(test)]
mod tests;

pub(crate) use pump::ProgressPump;

/// How many frames may wait for the pump before the queue starts dropping
/// them.
///
/// A frame is one section boundary, so sixty-four is far more than any run
/// produces in a burst: the bound is a backstop against a peer that stopped
/// reading, not a throttle on a healthy one.
const CAPACITY: usize = 64;

/// One queued progress notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The run's latched section count.
    progress: u32,
    /// The caption the client renders in place.
    message: String,
}

/// An [`Observer`] that reports a run to an MCP client as it goes.
///
/// Built either [`silent`](Self::silent), which reports nothing, or
/// [`reporting`](Self::reporting), which reports to a peer through a
/// [`ProgressPump`]. Both count the run's model turns, so the caller reads
/// [`turns`](Self::turns) whichever it built.
#[derive(Debug)]
pub(crate) struct McpObserver {
    /// The queue the pump drains, absent when the call carried no
    /// `progressToken` and there is therefore nothing to report to.
    frames: Option<Sender<Frame>>,
    /// How many section-start observations the run reported.
    completed: AtomicU32,
    /// The run's model round trips, as the run itself reported them.
    turns: AtomicU32,
    /// Frames the queue had no room for: the peer is reading, but slower than
    /// the run produces boundaries.
    dropped: AtomicU32,
    /// Frames dropped because the pump is gone: the peer stopped accepting and
    /// the receiving half closed. A different situation from a full queue, so
    /// it is counted apart rather than folded into `dropped`.
    disconnected: AtomicU32,
    /// Complete records retained only for runner lifecycle regressions.
    #[cfg(test)]
    records: Mutex<Vec<(String, String, String)>>,
}

impl McpObserver {
    /// An observer that counts a run's turns and reports nothing else.
    ///
    /// This is what a call carrying no `progressToken` uses: no channel, no
    /// pump, and a result identical to the reported path's.
    #[must_use]
    pub(crate) fn silent() -> McpObserver {
        McpObserver::over(None)
    }

    /// An observer that reports a run to `peer` under `token`, and the pump
    /// that delivers what it queues.
    ///
    /// The pump owns the receiving half and runs on its own task, so the run's
    /// path never awaits the peer. Drop the observer once the run has ended -
    /// that closes the queue - and then await [`ProgressPump::finish`], which
    /// is what keeps the last frame from racing the result.
    ///
    /// # Panics
    /// Panics when called outside a Tokio runtime, since the pump runs on a
    /// task and that task is spawned here.
    pub(crate) fn reporting(
        peer: Peer<RoleServer>,
        token: ProgressToken,
    ) -> (McpObserver, ProgressPump) {
        let (observer, frames) = McpObserver::queued();
        let pump = ProgressPump::spawn(frames, peer, token);
        (observer, pump)
    }

    /// How many completed model round trips have been observed so far.
    ///
    /// Zero before the first completed turn, and zero throughout a run that
    /// reaches no model at all.
    #[must_use]
    pub(crate) fn turns(&self) -> u32 {
        self.turns.load(Ordering::Relaxed)
    }

    /// How many frames were dropped because the queue was full.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many frames were dropped because the pump had already gone away.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn disconnected(&self) -> u32 {
        self.disconnected.load(Ordering::Relaxed)
    }

    /// Complete correlated observations retained by the test observer.
    #[cfg(test)]
    pub(crate) fn records(&self) -> Vec<(String, String, String)> {
        self.records
            .lock()
            .expect("the MCP test recorder mutex must not be poisoned")
            .clone()
    }

    /// An observer over `frames`, or over nothing.
    fn over(frames: Option<Sender<Frame>>) -> McpObserver {
        McpObserver {
            frames,
            completed: AtomicU32::new(0),
            turns: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
            disconnected: AtomicU32::new(0),
            #[cfg(test)]
            records: Mutex::new(Vec::new()),
        }
    }

    /// An observer and the queue it fills, without the task that drains it.
    ///
    /// [`reporting`](Self::reporting) is this plus that task; a test that wants
    /// to inspect the frames themselves, or to leave them undrained, is the
    /// other caller.
    fn queued() -> (McpObserver, Receiver<Frame>) {
        let (sender, receiver) = mpsc::channel(CAPACITY);
        (McpObserver::over(Some(sender)), receiver)
    }

    /// Increments and returns the number of sections entered.
    fn advance(&self) -> u32 {
        self.completed.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Queues one frame, or counts it as dropped.
    ///
    /// Never blocks: both a full queue and a pump that has gone away drop the
    /// frame rather than stall the run over a report, but the two are counted
    /// and logged apart. A full queue is a slow-but-present peer (backpressure);
    /// a closed queue is a peer that stopped accepting entirely.
    fn queue(&self, progress: u32, message: &str) {
        let Some(frames) = &self.frames else {
            return;
        };
        let frame = Frame {
            progress,
            message: message.to_owned(),
        };
        match frames.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!(
                    dropped,
                    progress,
                    "dropped a progress frame: the queue is full"
                );
            }
            Err(TrySendError::Closed(_)) => {
                let disconnected = self.disconnected.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!(
                    disconnected,
                    progress,
                    "dropped a progress frame: the pump is gone"
                );
            }
        }
    }
}

impl Observer for McpObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        #[cfg(test)]
        self.records
            .lock()
            .expect("the MCP test recorder mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), event.to_string()));
        // `execution`, `section`, and an unrecognized detail's payload are all
        // author-controlled per the Observer contract, so none is interpolated
        // into an operator log: a prompt must not be able to write the server's
        // trusted log stream. The test recorder above keeps them for lifecycle
        // assertions, and `section` still rides the dedicated progress channel
        // as a client-facing caption; the log carries only stable counts.
        let _ = execution;
        match event {
            Observation::RunStarted => {
                tracing::info!("run started");
                self.queue(0, section);
            }
            Observation::SectionStarted => {
                self.queue(self.advance(), section);
            }
            Observation::SectionFinished => {
                tracing::debug!("section finished");
            }
            Observation::ModelTurnCompleted => {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!(turn, "model turn completed");
            }
            Observation::ModelTurnFailed => {
                tracing::warn!("model turn failed");
            }
            Observation::ToolCallSucceeded => {
                tracing::debug!("tool call succeeded");
            }
            Observation::ToolCallFailed => {
                tracing::warn!("tool call failed");
            }
            Observation::RunSucceeded => {
                tracing::debug!("run success observed");
            }
            Observation::RunFailed => {
                tracing::debug!("run failure observed");
            }
            _other => {
                tracing::debug!("unrecognized observation");
            }
        }
    }
}
