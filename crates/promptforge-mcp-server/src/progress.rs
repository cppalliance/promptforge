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
//! one exception is a tool call that failed, which is `warn`: a search that
//! came back empty is a thing an operator wants without turning on debug, and
//! it is rare enough not to flood.
//!
//! [`RunResult`]: crate::RunResult
//!
//! `total` is never sent. A `goto` or an early return means the number of
//! sections a run will visit is unknown when it starts, so a denominator would
//! be a guess wearing a measurement's clothes; the client shows a changing
//! caption rather than a filling bar. `progress` is latched with a maximum, so
//! it never decreases, which the protocol requires.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use promptforge_core::observe::{Observer, detail};
use rmcp::RoleServer;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::Peer;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

/// How many frames may wait for the pump before the queue starts dropping
/// them.
///
/// A frame is one section boundary, so sixty-four is far more than any run
/// produces in a burst: the bound is a backstop against a peer that stopped
/// reading, not a throttle on a healthy one.
const CAPACITY: usize = 64;

/// How long the reply waits for the pump to drain before abandoning it.
///
/// Sending a notification resolves only once the transport has accepted it, so
/// an unbounded wait lets a peer that stalled its stream without closing it
/// hold the whole `tools/call` open. Progress is best-effort in both
/// directions: a caption nobody reads is not worth a reply nobody receives.
///
/// A quarter second is chosen against the two things it trades off. A healthy
/// stream, local or across a LAN, accepts a queued frame in single-digit
/// milliseconds, so this is generous enough that the last caption is not lost
/// to an ordinary hiccup. A stalled one costs the caller a quarter second on a
/// reply that already took minutes to earn, which no user perceives. The value
/// deliberately does not track [`CAPACITY`]: a queue depth and a wait are
/// unrelated quantities that happened to be written with the same number.
const FLUSH_GRACE: Duration = Duration::from_millis(250);

/// One queued progress notification.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
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
///
/// # Examples
/// ```
/// use promptforge_core::observe::{Observer, detail};
/// use promptforge_mcp_server::McpObserver;
///
/// let observer = McpObserver::silent();
/// observer.observe("Gather", detail::MODEL_TURN_COMPLETED);
/// assert_eq!(observer.turns(), 1);
/// ```
#[derive(Debug)]
pub struct McpObserver {
    /// The queue the pump drains, absent when the call carried no
    /// `progressToken` and there is therefore nothing to report to.
    frames: Option<Sender<Frame>>,
    /// How many section-start observations the run reported.
    completed: AtomicU32,
    /// The run's model round trips, as the run itself reported them.
    turns: AtomicU32,
    /// Frames the queue had no room for.
    dropped: AtomicU32,
}

impl McpObserver {
    /// An observer that counts a run's turns and reports nothing else.
    ///
    /// This is what a call carrying no `progressToken` uses: no channel, no
    /// pump, and a result identical to the reported path's.
    #[must_use]
    pub fn silent() -> McpObserver {
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
    pub fn reporting(peer: Peer<RoleServer>, token: ProgressToken) -> (McpObserver, ProgressPump) {
        let (observer, frames) = McpObserver::queued();
        let task = tokio::spawn(pump(frames, peer, token));
        (observer, ProgressPump { task })
    }

    /// How many completed model round trips have been observed so far.
    ///
    /// Zero before the first completed turn, and zero throughout a run that
    /// reaches no model at all.
    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns.load(Ordering::Relaxed)
    }

    /// How many frames were dropped because the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// An observer over `frames`, or over nothing.
    fn over(frames: Option<Sender<Frame>>) -> McpObserver {
        McpObserver {
            frames,
            completed: AtomicU32::new(0),
            turns: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
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
    /// Never blocks: a full queue and a pump that has gone away are both a
    /// dropped frame, because the alternative is stalling the run over a
    /// report.
    fn queue(&self, progress: u32, message: &str) {
        let Some(frames) = &self.frames else {
            return;
        };
        let frame = Frame {
            progress,
            message: message.to_owned(),
        };
        if frames.try_send(frame).is_err() {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::debug!(dropped, progress, "dropped a progress frame");
        }
    }
}

impl Observer for McpObserver {
    fn observe(&self, section: &str, report: &str) {
        match report {
            detail::RUN_STARTED => {
                tracing::info!(%section, "run started");
                self.queue(0, section);
            }
            detail::SECTION_STARTED => {
                self.queue(self.advance(), section);
            }
            detail::SECTION_FINISHED => {
                tracing::debug!(%section, "section finished");
            }
            detail::MODEL_TURN_COMPLETED => {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!(%section, turn, "model turn completed");
            }
            detail::MODEL_TURN_FAILED => {
                tracing::warn!(%section, "model turn failed");
            }
            detail::TOOL_CALL_SUCCEEDED => {
                tracing::debug!(%section, "tool call succeeded");
            }
            detail::TOOL_CALL_FAILED => {
                tracing::warn!(%section, "tool call failed");
            }
            detail::RUN_SUCCEEDED => {
                tracing::debug!(%section, "run success observed");
            }
            detail::RUN_FAILED => {
                tracing::debug!(%section, "run failure observed");
            }
            unknown => {
                tracing::debug!(%section, detail = %unknown, "unrecognized observation");
            }
        }
    }
}

/// The task that delivers what an [`McpObserver`] queues.
///
/// Ends when the observer is dropped, which closes the queue, or when the peer
/// stops accepting notifications.
#[derive(Debug)]
#[must_use = "the frames a run queued are delivered by the pump, so finish it"]
pub struct ProgressPump {
    /// The forwarding task.
    task: JoinHandle<()>,
}

impl ProgressPump {
    /// Waits a bounded grace period for every frame queued before the observer
    /// was dropped, then abandons what is left.
    ///
    /// Awaiting this before answering the call is what keeps the last frame
    /// from arriving after the result it described. The wait is bounded
    /// because a peer that has stopped reading its stream would otherwise hold
    /// the reply open for as long as it liked.
    pub async fn finish(mut self) {
        match tokio::time::timeout(FLUSH_GRACE, &mut self.task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::debug!(%error, "the progress pump did not finish cleanly");
            }
            Err(_elapsed) => {
                self.task.abort();
                tracing::debug!("abandoned the progress pump, which was still sending");
            }
        }
    }
}

/// Forwards frames to the client until the queue closes or the peer refuses
/// one.
async fn pump(mut frames: Receiver<Frame>, peer: Peer<RoleServer>, token: ProgressToken) {
    while let Some(frame) = frames.recv().await {
        let notification = ProgressNotificationParam::new(token.clone(), f64::from(frame.progress))
            .with_message(frame.message);
        if let Err(error) = peer.notify_progress(notification).await {
            tracing::debug!(%error, "stopped reporting progress");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::{Frame, McpObserver, ProgressPump, Receiver};
    use crate::levels::Levels;
    use promptforge_core::execute::{self, RunOptions};
    use promptforge_core::observe::{Observer, detail};
    use promptforge_core::parser::Prompt;
    use promptforge_core::store::Store;
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    /// Every frame the queue is holding.
    fn drain(frames: &mut Receiver<Frame>) -> Vec<Frame> {
        let mut drained = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            drained.push(frame);
        }
        drained
    }

    /// The frame a caption and a count would produce.
    fn frame(progress: u32, message: &str) -> Frame {
        Frame {
            progress,
            message: message.to_owned(),
        }
    }

    /// A prompt of `sections` Lua-only sections, the last of which returns, so
    /// the whole run happens offline and emits one frame per section.
    fn long_prompt(sections: usize) -> String {
        let mut source = String::from(
            "---\nname: long\ndescription: Many sections\nversion: 1\npromptforge: 1\n---\n",
        );
        for section in 1..sections {
            let _written = write!(
                source,
                "\n## S{section}\n\n```lua\nvar.step = {section}\n```\n"
            );
        }
        let _written = write!(
            source,
            "\n## S{sections}\n\n```lua\nreturn 'long done'\n```\n"
        );
        source
    }

    /// The observations a three-section run emits, one section of which takes a
    /// model turn and a tool call.
    fn three_section_run() -> Vec<(&'static str, &'static str)> {
        vec![
            ("Trio", detail::RUN_STARTED),
            ("First", detail::SECTION_STARTED),
            ("First", detail::MODEL_TURN_COMPLETED),
            ("First", detail::TOOL_CALL_SUCCEEDED),
            ("First", detail::SECTION_FINISHED),
            ("Second", detail::SECTION_STARTED),
            ("Second", detail::SECTION_FINISHED),
            ("Third", detail::SECTION_STARTED),
            ("Third", detail::SECTION_FINISHED),
            ("Trio", detail::RUN_SUCCEEDED),
        ]
    }

    #[test]
    fn a_run_frames_its_start_and_each_section_and_nothing_else() {
        let (observer, mut frames) = McpObserver::queued();
        for (section, report) in three_section_run() {
            observer.observe(section, report);
        }
        assert_eq!(
            drain(&mut frames),
            vec![
                frame(0, "Trio"),
                frame(1, "First"),
                frame(2, "Second"),
                frame(3, "Third"),
            ]
        );
        assert_eq!(observer.turns(), 1, "the run's own total is what is kept");
        assert_eq!(observer.dropped(), 0);
    }

    #[test]
    fn progress_counts_recognized_section_starts() {
        let (observer, mut frames) = McpObserver::queued();
        for section in ["one", "two", "three"] {
            observer.observe(section, detail::SECTION_STARTED);
        }
        let progress: Vec<u32> = drain(&mut frames).iter().map(|f| f.progress).collect();
        assert_eq!(progress, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn a_pump_that_never_drains_still_lets_the_run_finish() {
        // `frames` is held and never read, which is a pump whose peer has
        // stopped accepting: the queue fills, and the run must not notice.
        let sections = super::CAPACITY + 16;
        let source = long_prompt(sections);
        let prompt = Prompt::parse(&source).expect("the fixture prompt parses");
        let (observer, _frames) = McpObserver::queued();

        let store = Store::memory();
        let options = RunOptions {
            observer: &observer,
            client: None,
        };
        let value = execute::run(&prompt, "", &[], &store, options)
            .await
            .expect("a Lua-only run reaches no model and finishes");

        assert_eq!(value, "long done");
        assert!(
            observer.dropped() > 0,
            "a run past the queue's capacity drops frames rather than stalling"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pump_the_peer_never_accepts_is_abandoned() {
        // A send that never resolves is what a peer holding its stream open
        // produces, and the reply must not wait on it.
        let pump = ProgressPump {
            task: tokio::spawn(std::future::pending()),
        };
        let started = tokio::time::Instant::now();
        pump.finish().await;
        assert_eq!(
            started.elapsed(),
            super::FLUSH_GRACE,
            "the flush waits its grace and no longer"
        );
    }

    #[test]
    fn only_the_run_start_and_a_failed_tool_call_reach_the_default_level() {
        let levels = Levels::default();
        let subscriber = tracing_subscriber::registry().with(levels.clone());
        let observer = McpObserver::silent();
        tracing::subscriber::with_default(subscriber, || {
            for (section, report) in three_section_run() {
                observer.observe(section, report);
            }
            observer.observe("First", detail::TOOL_CALL_FAILED);
        });
        assert_eq!(
            levels.operator_visible(),
            vec![Level::INFO, Level::WARN],
            "the run start, then the failed tool call"
        );
    }

    #[test]
    fn a_silent_observer_counts_turns_without_a_queue() {
        let observer = McpObserver::silent();
        for (section, report) in three_section_run() {
            observer.observe(section, report);
        }
        assert_eq!(observer.turns(), 1);
        assert_eq!(observer.dropped(), 0, "there is nothing to drop into");
    }

    #[test]
    fn unknown_details_are_tolerated_without_frames_or_counters() {
        let (observer, mut frames) = McpObserver::queued();
        observer.observe("First", "A future detail");

        assert!(drain(&mut frames).is_empty());
        assert_eq!(observer.turns(), 0);
        assert_eq!(observer.dropped(), 0);
    }
}
