//! The tokio test driver: a loop over [`Run`] that performs its effects on
//! a tokio runtime through a caller's [`Performers`] and hands every event
//! to a caller's sink.
//!
//! The loop is `step -> perform -> await an answer -> resume`. Every
//! `Chat` and `ToolCall` effect the step hands out goes to
//! the matching performer closure, whose future is spawned as one task
//! that posts its answer on a channel under the effect's id; the loop
//! resumes the run with each arriving answer and steps again. The
//! driver performs the engine-internal kinds itself: a `Vfs` operation
//! runs on the blocking pool (the VFS is synchronous by design), and a
//! `Timer` sleeps on tokio's timer wheel.
//!
//! When the run reports itself decided ([`Run::decided`]) every performer
//! still out is aborted and joined - a blocking-pool store operation runs
//! to completion before the run ends - and its effect is answered
//! `Dropped`, as is every effect issued in the deciding step itself, which
//! is never performed; so the run reaches `Done` with every effect
//! answered exactly once.
//!
//! Cancellation is a synchronous flag: the caller hands one to
//! [`drive_tokio`], the loop awaits it beside the answer channel, and when
//! it fires the loop cancels the run so a run whose chains are all
//! suspended tears down promptly. Running Lua observes the run's own flag
//! from its instruction hook.
//!
//! Under a test shuffle seed (the test-only `set_shuffle_for_test`) the
//! loop instead holds each wave of answers until every outstanding effect
//! has posted one, then delivers the wave in the seed's permutation: one
//! seed is one deterministic completion interleaving, which the
//! determinism suites sweep across seeds. The performers all post
//! independently of the run, so the hold cannot deadlock.
//!
//! This driver plays the Harness's part in tests: the Engine's own suites
//! drive runs through it, and a companion crate's suite enables the
//! `test-support` feature for it. In production the Harness steps the run.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use promptforge_types::event::Event;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cancel::CancelHandle;
use crate::lua::run_store_op;
#[cfg(test)]
use crate::test_support::scripted_chat::ScriptedChat;
use crate::{Error, Result};

#[cfg(test)]
use crate::execute::EffectRecord;
use crate::execute::{Effect, EffectAnswer, EffectId, Run, RunResult, Step};
#[cfg(test)]
use crate::test_support::RunHarness;

#[cfg(test)]
use crate::execute::context::RunState;

#[path = "tokio_driver-performers.rs"]
mod performers;
#[cfg(test)]
#[path = "tokio_driver-test-hooks.rs"]
mod test_hooks;

pub(crate) use performers::refuse_tool_call;
pub use performers::{BoxFuture, Performer, Performers};
#[cfg(test)]
use test_hooks::shuffle_batch;

/// The sink every drained event is handed to, in step order.
pub(crate) type EventSink<'a> = Box<dyn FnMut(Event) + Send + 'a>;

/// Drives `run` to its end on the current tokio runtime, performing its
/// `Chat` and `ToolCall` effects through `performers`,
/// handing every event to `sink` in order, and cancelling the run when
/// `cancel` fires. Returns the run's result.
///
/// The future is boxed internally: the step machinery is large, and the
/// caller's own future stays small.
///
/// # Examples
/// A prompt whose only section returns a literal issues no effect, so
/// the refusing performers are never called:
/// ```
/// use std::sync::Arc;
///
/// use promptforge::cancel::CancelHandle;
/// use promptforge_engine::test_support::{Performers, drive_tokio};
/// use promptforge::timestamp::Timestamp;
/// use promptforge::{Prompt, Run, RunContext, RunResult};
///
/// let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n## Only\n\n```lua\nreturn 'hello'\n```\n";
/// let (prompt, _parse_events) = Prompt::parse(source, "doc-example");
/// let prompt = prompt?;
/// let ctx = RunContext::new("doc-example", 1, Timestamp::UNIX_EPOCH);
/// let run = Run::new(Arc::new(prompt), "", ctx);
/// let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
/// let mut events = Vec::new();
/// let result = runtime.block_on(drive_tokio(
///     run,
///     Performers::refusing(),
///     |event| events.push(event),
///     CancelHandle::new(),
/// ));
/// let RunResult::Ok(text) = result else {
///     panic!("the literal run succeeds: {result:?}");
/// };
/// assert_eq!(text, "hello");
/// assert!(!events.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub async fn drive_tokio(
    run: Run,
    performers: Performers,
    sink: impl FnMut(Event) + Send,
    cancel: CancelHandle,
) -> RunResult {
    let mut driver = TokioDriver::over(run, performers, Box::new(sink), cancel);
    let result = Box::pin(driver.drive()).await;
    match result {
        Ok(text) => RunResult::Ok(text),
        Err(Error::Interrupted) => RunResult::Cancelled,
        Err(error) => RunResult::Failure(crate::execute::RunError::from(error)),
    }
}

/// The send half every performer posts its answer to.
type AnswerSender = mpsc::UnboundedSender<(EffectId, EffectAnswer)>;

/// One run driven on tokio.
pub(crate) struct TokioDriver<'a> {
    /// The run being driven.
    run: Run,
    /// The caller's performers for the kinds it performs.
    performers: Performers,
    /// Where every drained event goes.
    sink: EventSink<'a>,
    /// The answer channel: unbounded, because each performer sends exactly
    /// once and the in-flight count is already bounded by the chains that
    /// produced the effects.
    tx: AnswerSender,
    rx: mpsc::UnboundedReceiver<(EffectId, EffectAnswer)>,
    /// The performers still out, keyed by effect. An answer for an id not
    /// here is a late answer for an effect already dropped and is
    /// discarded, so the run never sees two answers for one effect.
    outstanding: HashMap<EffectId, JoinHandle<()>>,
    /// The caller's cancel flag, awaited while the loop waits on answers;
    /// when it fires the run is cancelled.
    cancel: CancelHandle,
    /// Test-only: the seed for the completion-order shuffle; `None`
    /// delivers answers in arrival order.
    #[cfg(test)]
    shuffle: Option<u64>,
    /// Test-only: the record of every effect performed, in issue order.
    #[cfg(test)]
    tap: Option<Arc<Mutex<Vec<EffectRecord>>>>,
}

impl<'a> TokioDriver<'a> {
    /// Builds the driver for one run over `state`, performing its effects
    /// and replaying its events through the `harness` the suite assembled
    /// itself. The bundle supplies the observer, chat client, tools, delta
    /// hook, and debug capture; `client` is the run's scripted model when
    /// the suite supplies one, overriding any in the bundle.
    #[cfg(test)]
    pub(crate) fn new(
        state: &RunState,
        harness: RunHarness,
        client: Option<ScriptedChat>,
    ) -> TokioDriver<'static> {
        let mut harness = harness;
        if let Some(client) = client {
            harness = harness.client(client);
        }
        let run = Run::from_state(state.clone());
        let cancel = run.cancel_handle();
        let limits = state.limits();
        TokioDriver::over(
            run,
            harness.performers(limits),
            harness.boxed_sink(),
            cancel,
        )
    }

    /// Builds the driver over an assembled run.
    pub(crate) fn over(
        run: Run,
        performers: Performers,
        sink: EventSink<'a>,
        cancel: CancelHandle,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            run,
            performers,
            sink,
            tx,
            rx,
            outstanding: HashMap::new(),
            cancel,
            #[cfg(test)]
            tap: None,
            #[cfg(test)]
            shuffle: None,
        }
    }

    /// Drives the run to its end and returns its result as the Engine's
    /// own error type: `Ok(text)` for a completed run, `Err(Interrupted)`
    /// for a cancelled one, and the failure's error otherwise.
    ///
    /// # Errors
    /// Returns the [`Error`] the run failed with, or [`Error::Interrupted`]
    /// when it was cancelled.
    pub(crate) async fn drive(&mut self) -> Result<String> {
        loop {
            match self.run.step() {
                Step::Done { result, events } => {
                    self.forward(events);
                    // `Done` is returned only once every effect is
                    // answered, so nothing is out; the map is empty.
                    return match result {
                        RunResult::Ok(text) => Ok(text),
                        RunResult::Cancelled => Err(Error::Interrupted),
                        RunResult::Failure(error) => Err(error.into_inner()),
                    };
                }
                Step::Pending { effects, events } => {
                    // The run's own word, not a scan of its events: the
                    // events are a report only.
                    let decided = self.run.decided();
                    self.forward(events);
                    #[cfg(test)]
                    self.record(&effects);
                    if decided {
                        // The run has decided; every effect it issued in
                        // this step is moot before it is performed, and
                        // every performer still out is moot too. Answer
                        // them all `Dropped` so the next step reaches
                        // `Done`, and perform nothing that the run has
                        // already stopped waiting for.
                        for (id, _, _) in effects {
                            self.run.resume(id, EffectAnswer::Dropped);
                        }
                        self.drop_outstanding().await;
                        continue;
                    }
                    for (id, _, effect) in effects {
                        self.perform(id, effect);
                    }
                    if self.outstanding.is_empty() {
                        // The run reports a stall itself; reaching here
                        // means the driver lost a performer.
                        return Err(Error::internal(
                            "the driver has nothing to await for a pending run",
                        ));
                    }
                    self.await_answer().await;
                }
            }
        }
    }

    /// Waits for the next answer, applying every answer already queued
    /// behind it, or returns as soon as the cancel flag is set - cancelling
    /// the run so its next step observes it. Both arms are event-driven:
    /// the channel wakes on a posted answer and the flag's future wakes on
    /// the cancel, so a fully suspended run costs no wakeups while it
    /// waits.
    async fn await_answer(&mut self) {
        #[cfg(test)]
        if self.shuffle.is_some() {
            self.await_shuffled_batch().await;
            return;
        }
        tokio::select! {
            biased;
            arrival = self.rx.recv() => {
                let mut batch = Vec::new();
                if let Some(pair) = arrival {
                    batch.push(pair);
                }
                while let Ok(pair) = self.rx.try_recv() {
                    batch.push(pair);
                }
                self.deliver_batch(batch);
            }
            // The run acts on its own flag at its next step; setting it
            // here is what makes that step happen promptly when the
            // caller's flag is a different handle.
            () = self.cancel.cancelled() => {
                self.run.cancel();
            }
        }
    }

    /// Delivers one wave of answers in arrival order, or - under a test
    /// shuffle seed - in the seed's permutation of the wave.
    fn deliver_batch(&mut self, batch: Vec<(EffectId, EffectAnswer)>) {
        #[cfg(test)]
        let batch = match &mut self.shuffle {
            Some(seed) => shuffle_batch(batch, seed),
            None => batch,
        };
        for (id, answer) in batch {
            self.deliver(id, answer);
        }
    }

    /// Resumes the run with one performer's answer, unless the effect was
    /// already dropped, in which case the late answer is discarded.
    fn deliver(&mut self, id: EffectId, answer: EffectAnswer) {
        if self.outstanding.remove(&id).is_some() {
            self.run.resume(id, answer);
        }
    }

    /// Aborts and joins every performer still out and answers each of
    /// their effects `Dropped`. A blocking-pool store operation cannot be
    /// interrupted, so the join waits for it to finish; only then does the
    /// run reach `Done`, with every issued effect answered exactly once.
    async fn drop_outstanding(&mut self) {
        let outstanding = std::mem::take(&mut self.outstanding);
        for (id, handle) in outstanding {
            handle.abort();
            let _ = handle.await;
            self.run.resume(id, EffectAnswer::Dropped);
        }
        // Whatever the joined performers posted before the abort is stale:
        // their effects are answered.
        while self.rx.try_recv().is_ok() {}
    }

    /// Hands one step's events to the sink.
    fn forward(&mut self, events: Vec<Event>) {
        for event in events {
            (self.sink)(event);
        }
    }

    /// Performs one effect: spawns the performer that will post the
    /// effect's answer under `id`.
    fn perform(&mut self, id: EffectId, effect: Effect) {
        let tx = self.tx.clone();
        let handle = match effect {
            Effect::Chat { .. } => {
                let future = (self.performers.chat)(effect);
                tokio::spawn(async move { post(&tx, id, future.await) })
            }
            Effect::ToolCall { .. } => {
                let future = (self.performers.tool_call)(effect);
                tokio::spawn(async move { post(&tx, id, future.await) })
            }
            Effect::Vfs { access, op } => {
                // spawn_blocking, not a plain task: the Vfs is sync by
                // design, and the blocking pool keeps a slow real-filesystem
                // op from stalling the loop. Aborting the handle detaches
                // rather than interrupts, so a dropped op completes before
                // its join returns.
                tokio::task::spawn_blocking(move || {
                    let result = run_store_op(&access, op);
                    // Hygiene only, as in the Harness's inline Vfs answer:
                    // the run ends its scope at `Done`, so a post-run
                    // fresh-scope read never meets the run's claims
                    // however long the view is held.
                    drop(access);
                    post(&tx, id, EffectAnswer::Vfs(result));
                })
            }
            Effect::Timer { seconds } => {
                // The arm bounds the seconds; a duration the wheel cannot
                // hold fires at once rather than never.
                let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
                tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    post(&tx, id, EffectAnswer::Timer);
                })
            }
        };
        self.outstanding.insert(id, handle);
    }
}

impl std::fmt::Debug for TokioDriver<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokioDriver")
            .field("run", &self.run)
            .field("outstanding", &self.outstanding.len())
            .finish_non_exhaustive()
    }
}

/// Aborts every performer still out when the driver is dropped
/// mid-run - a suite tearing the run down without driving it to its end.
/// Dropping a bare `JoinHandle` detaches the task, which would strand a
/// tool call or model round forever, so the drop applies the same
/// abort the run's end does.
impl Drop for TokioDriver<'_> {
    fn drop(&mut self) {
        for handle in self.outstanding.values() {
            handle.abort();
        }
    }
}

/// Posts one answer. A send fails only when the driver is gone (a dropped
/// driver whose receiver closed); the answer is then moot.
fn post(tx: &AnswerSender, id: EffectId, answer: EffectAnswer) {
    let _ = tx.send((id, answer));
}
