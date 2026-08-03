//! Progress for a run in flight: the observer that queues it, the pump that
//! sends it.
//!
//! One `tools/call` can take minutes, and `notifications/progress` is the only
//! thing a client can render while it waits. The two halves of that job have
//! incompatible shapes: the core reports a run through [`Observer`], whose
//! `on_event` is synchronous and sits on the run's own path, while sending a
//! notification is an `.await` on the peer. [`McpObserver`] is the join between
//! them. `on_event` turns an event into a frame and hands it to a bounded
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
//! What each event produces:
//!
//! | Event | Frame |
//! |---|---|
//! | `RunStarted` | frame 0, captioned with the prompt's name |
//! | `SectionStarted` | the latched `completed`, captioned with the section |
//! | `SectionFinished`, `ModelTurn`, `ToolCalled` | none; logged only |
//! | `RunFinished` | none; it latches the run's turn total |
//!
//! `SectionFinished` sends nothing because the frame it would send is the one
//! already on the wire: same latched `progress`, same section name. A client
//! renders the caption in place, so a second identical frame is invisible to
//! the reader and pure cost on the stream.
//!
//! What each event logs, which is a separate question from what it sends: the
//! two run boundaries are `info`, so an operator at the default level sees that
//! a run happened, what it was, how long it took, and how many model turns it
//! needed - a run that quietly takes too long is this service's characteristic
//! failure, and those two lines are what make it visible. Everything inside a
//! run is `debug`, because a section can make dozens of tool calls and burying
//! the boundaries under them defeats the purpose. The one exception is a tool
//! call that failed, which is `warn`: a search that came back empty is a thing
//! an operator wants without turning on debug, and it is rare enough not to
//! flood.
//!
//! `total` is never sent. A `goto` or an early return means the number of
//! sections a run will visit is unknown when it starts, so a denominator would
//! be a guess wearing a measurement's clothes; the client shows a changing
//! caption rather than a filling bar. `progress` is latched with a maximum, so
//! it never decreases, which the protocol requires.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use promptforge_core::observe::{Event, Observer};
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
/// use promptforge_core::observe::{Event, Observer};
/// use promptforge_mcp::McpObserver;
///
/// let observer = McpObserver::silent();
/// observer.on_event(&Event::RunFinished { turns: 3, elapsed_ms: 40, ok: true });
/// assert_eq!(observer.turns(), 3);
/// ```
#[derive(Debug)]
pub struct McpObserver {
    /// The queue the pump drains, absent when the call carried no
    /// `progressToken` and there is therefore nothing to report to.
    frames: Option<Sender<Frame>>,
    /// The highest `completed` any section reported, which is what makes
    /// `progress` monotonic.
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

    /// How many model round trips the run reported taking.
    ///
    /// Zero until the run ends, and zero after it for a run that reached no
    /// model at all.
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

    /// Raises the latch to `completed` and returns its new value, so a section
    /// reported out of order repeats a number rather than going backwards.
    fn latch(&self, completed: u32) -> u32 {
        self.completed
            .fetch_max(completed, Ordering::Relaxed)
            .max(completed)
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
    fn on_event(&self, ev: &Event) {
        match ev {
            Event::RunStarted { prompt, sections } => {
                tracing::info!(%prompt, sections, "run started");
                self.queue(0, prompt);
            }
            Event::SectionStarted { completed, name } => {
                let progress = self.latch(*completed);
                self.queue(progress, name);
            }
            Event::SectionFinished { name } => {
                tracing::debug!(section = %name, "section finished");
            }
            Event::ModelTurn { section, turn } => {
                tracing::debug!(section = %section, turn, "model turn");
            }
            Event::ToolCalled { section, tool, ok } => {
                if *ok {
                    tracing::debug!(section = %section, tool = %tool, ok, "tool called");
                } else {
                    tracing::warn!(section = %section, tool = %tool, ok, "tool call failed");
                }
            }
            Event::RunFinished {
                turns,
                elapsed_ms,
                ok,
            } => {
                self.turns.store(*turns, Ordering::Relaxed);
                tracing::info!(turns, elapsed_ms, ok, "run finished");
            }
            // `Event` is `#[non_exhaustive]`, so a variant added upstream
            // reaches here rather than breaking the build. An unreported event
            // is the right default: a frame's meaning is this crate's to
            // decide, not the core's to impose.
            other => tracing::debug!(?other, "unreported run event"),
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
    use promptforge_core::observe::{Event, Observer};
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

    /// The events a three-section run emits, one section of which takes a model
    /// turn and a tool call.
    fn three_section_run() -> Vec<Event> {
        vec![
            Event::RunStarted {
                prompt: "trio".to_string(),
                sections: 3,
            },
            Event::SectionStarted {
                completed: 1,
                name: "First".to_string(),
            },
            Event::ModelTurn {
                section: "First".to_string(),
                turn: 1,
            },
            Event::ToolCalled {
                section: "First".to_string(),
                tool: "WebSearch".to_string(),
                ok: true,
            },
            Event::SectionFinished {
                name: "First".to_string(),
            },
            Event::SectionStarted {
                completed: 2,
                name: "Second".to_string(),
            },
            Event::SectionFinished {
                name: "Second".to_string(),
            },
            Event::SectionStarted {
                completed: 3,
                name: "Third".to_string(),
            },
            Event::SectionFinished {
                name: "Third".to_string(),
            },
            Event::RunFinished {
                turns: 1,
                elapsed_ms: 40,
                ok: true,
            },
        ]
    }

    #[test]
    fn a_run_frames_its_start_and_each_section_and_nothing_else() {
        let (observer, mut frames) = McpObserver::queued();
        for ev in three_section_run() {
            observer.on_event(&ev);
        }
        assert_eq!(
            drain(&mut frames),
            vec![
                frame(0, "trio"),
                frame(1, "First"),
                frame(2, "Second"),
                frame(3, "Third"),
            ]
        );
        assert_eq!(observer.turns(), 1, "the run's own total is what is kept");
        assert_eq!(observer.dropped(), 0);
    }

    #[test]
    fn progress_never_decreases() {
        let (observer, mut frames) = McpObserver::queued();
        for completed in [1, 3, 2] {
            observer.on_event(&Event::SectionStarted {
                completed,
                name: format!("s{completed}"),
            });
        }
        let progress: Vec<u32> = drain(&mut frames).iter().map(|f| f.progress).collect();
        assert_eq!(progress, vec![1, 3, 3], "a repeat repeats its number");
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
    fn only_the_run_boundaries_and_a_failed_tool_call_reach_the_default_level() {
        let levels = Levels::default();
        let subscriber = tracing_subscriber::registry().with(levels.clone());
        let observer = McpObserver::silent();
        tracing::subscriber::with_default(subscriber, || {
            for ev in three_section_run() {
                observer.on_event(&ev);
            }
            observer.on_event(&Event::ToolCalled {
                section: "First".to_string(),
                tool: "WebSearch".to_string(),
                ok: false,
            });
        });
        assert_eq!(
            levels.operator_visible(),
            vec![Level::INFO, Level::INFO, Level::WARN],
            "the two run boundaries, then the failed tool call"
        );
    }

    #[test]
    fn a_silent_observer_counts_turns_without_a_queue() {
        let observer = McpObserver::silent();
        for ev in three_section_run() {
            observer.on_event(&ev);
        }
        assert_eq!(observer.turns(), 1);
        assert_eq!(observer.dropped(), 0, "there is nothing to drop into");
    }
}
