//! The tokio test driver: a loop over [`Run`] that performs its effects on
//! a tokio runtime through a caller's [`Performers`] and hands every event
//! to a caller's sink.
//!
//! The loop is `step -> perform -> await an answer -> resume`. Every
//! `Chat`, `ToolCall`, and `UserInput` effect the step hands out goes to
//! the matching performer closure, whose future is spawned as one task
//! that posts its answer on a channel under the effect's id; the loop
//! resumes the run with each arriving answer and steps again. The
//! driver performs the engine-internal kinds itself: a `Store` operation
//! runs on the blocking pool (the VFS is synchronous by design), a `Timer`
//! sleeps on tokio's timer wheel, and a `TaskEvents` read is answered at
//! issue from the driver's own history of forwarded events.
//!
//! When the run reports itself decided ([`Run::decided`]) every performer
//! still out is aborted and joined - a blocking-pool store operation runs
//! to completion, so its access clone and the claims it holds release
//! before the result is delivered - and its effect is answered `Dropped`,
//! as is every effect issued in the deciding step itself, which is never
//! performed; so the run reaches `Done` with every effect answered exactly
//! once.
//!
//! Cancellation is a synchronous flag: the caller hands one to
//! [`drive_tokio`], the loop awaits it beside the answer channel, and when
//! it fires the loop cancels the run so a run whose chains are all
//! suspended tears down promptly. Running Lua observes the run's own flag
//! from its instruction hook.
//!
//! This is a test host: the engine's own suites drive it in place of the
//! scheduler they used to drive, and a companion crate's suite enables
//! the `test-support` feature for it. The harness is the production host.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use promptforge_types::event::Event;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cancel::CancelHandle;
use crate::lua::run_store_op;
use crate::store::Store;
#[cfg(test)]
use crate::test_support::mock_gateway_client::MockGatewayClient;
use crate::{Error, Result};

#[cfg(test)]
use crate::execute::EffectRecord;
use crate::execute::{Effect, EffectAnswer, EffectId, Run, RunResult, Step, task_history};
#[cfg(test)]
use crate::test_support::RunHost;

#[cfg(test)]
use crate::execute::context::RunState;
#[cfg(test)]
use crate::execute::scheduler::Scheduler;

#[path = "tokio_driver-performers.rs"]
mod performers;

pub(crate) use performers::refuse_tool_call;
pub use performers::{BoxFuture, Performer, Performers};

/// The sink every drained event is handed to, in step order.
pub(crate) type EventSink<'a> = Box<dyn FnMut(Event) + Send + 'a>;

/// Drives `run` to its end on the current tokio runtime, performing its
/// `Chat`, `ToolCall`, and `UserInput` effects through `performers`,
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
/// use promptforge::test_support::{Performers, drive_tokio};
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
    /// The host's performers for the kinds it performs.
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
    /// Every event the run has reported, in step order: the history a
    /// `TaskEvents` effect is answered from. A step's events are appended
    /// before its effects are performed, so a task reading its own record
    /// sees everything reported before the read.
    history: Vec<Event>,
    /// Test-only: the record of every effect performed, in issue order.
    #[cfg(test)]
    tap: Option<Arc<Mutex<Vec<EffectRecord>>>>,
}

impl<'a> TokioDriver<'a> {
    /// Builds the driver for one run over `state`, performing its effects
    /// and replaying its events through the `host` the suite assembled
    /// itself. The host supplies the observer, broker, tools, delta hook,
    /// and debug capture; `client` is the run's mock-gateway client when
    /// the suite supplies one, overriding any on the host.
    #[cfg(test)]
    pub(crate) fn new(
        state: &RunState,
        host: RunHost,
        client: Option<MockGatewayClient>,
    ) -> TokioDriver<'static> {
        let mut host = host;
        if let Some(client) = client {
            host = host.client(client);
        }
        let run = Run::from_state(state.clone());
        let cancel = run.cancel_handle();
        let limits = state.limits();
        TokioDriver::over(run, host.performers(limits), host.boxed_sink(), cancel)
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
            history: Vec::new(),
            #[cfg(test)]
            tap: None,
        }
    }

    /// Drives the run to its end and returns its result as the engine's
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
                    let mut answered_inline = false;
                    for (id, _, effect) in effects {
                        answered_inline |= !self.perform(id, effect);
                    }
                    if answered_inline {
                        // An effect answered at issue re-queued its chain:
                        // step again before waiting on anything.
                        continue;
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
        tokio::select! {
            biased;
            arrival = self.rx.recv() => {
                if let Some((id, answer)) = arrival {
                    self.deliver(id, answer);
                }
                while let Ok((id, answer)) = self.rx.try_recv() {
                    self.deliver(id, answer);
                }
            }
            // The run acts on its own flag at its next step; setting it
            // here is what makes that step happen promptly when the
            // caller's flag is a different handle.
            () = self.cancel.cancelled() => {
                self.run.cancel();
            }
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
    /// interrupted, so the join waits for it to finish; only then is its
    /// access clone - and the claims it holds - gone, which is what keeps
    /// claim release bounded to the run's lifetime.
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

    /// Hands one step's events to the sink and appends them to the history
    /// `TaskEvents` reads answer from.
    fn forward(&mut self, events: Vec<Event>) {
        for event in events {
            (self.sink)(event.clone());
            self.history.push(event);
        }
    }

    /// Performs one effect: spawns the performer that will post the
    /// effect's answer under `id` and returns `true`, or answers at once
    /// and returns `false` for a `TaskEvents` read, which is answered from
    /// the history.
    fn perform(&mut self, id: EffectId, effect: Effect) -> bool {
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
            Effect::UserInput { .. } => {
                let future = (self.performers.user_input)(effect);
                tokio::spawn(async move { post(&tx, id, future.await) })
            }
            Effect::Store { access, op } => {
                // spawn_blocking, not a plain task: the Vfs is sync by
                // design, and the blocking pool keeps a slow host-backend
                // op from stalling the loop. Aborting the handle detaches
                // rather than interrupts, so a dropped op completes before
                // its join returns.
                tokio::task::spawn_blocking(move || {
                    let result = run_store_op(&Store::new(&access), op);
                    // Claims-release ordering constraint: the access clone
                    // must drop after the op and before the answer posts,
                    // so the claims it holds release before a resumed chain
                    // can acquire overlapping claims; the fix changes when
                    // claims release, never whether an operation succeeds.
                    drop(access);
                    post(&tx, id, EffectAnswer::Store(result));
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
            Effect::TaskEvents { task, last } => {
                // Answered from the driver's own history, at issue: the
                // step's events are already appended, so the read sees
                // everything reported before it.
                let events = task_history(&self.history, &task, last);
                self.run.resume(id, EffectAnswer::TaskEvents(events));
                return false;
            }
        };
        self.outstanding.insert(id, handle);
        true
    }

    /// Records every issued effect's record from here on, performed or
    /// dropped at issue.
    #[cfg(test)]
    pub(crate) fn record_effects_for_test(&mut self) -> Arc<Mutex<Vec<EffectRecord>>> {
        let tap = Arc::new(Mutex::new(Vec::new()));
        self.tap = Some(Arc::clone(&tap));
        tap
    }

    /// Appends one step's issued effects to the tap, in issue order.
    #[cfg(test)]
    fn record(&self, effects: &[(EffectId, promptforge_types::ids::Provenance, Effect)]) {
        if let Some(tap) = &self.tap {
            tap.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(effects.iter().map(|(_, _, effect)| effect.record()));
        }
    }

    /// The scheduler behind the run, for the suites that inspect its
    /// arena.
    #[cfg(test)]
    pub(crate) fn scheduler_for_test(&mut self) -> &mut Scheduler {
        self.run.scheduler_for_test()
    }

    /// The state of one task's slot, read through the scheduler.
    #[cfg(test)]
    pub(crate) fn task_state_for_test(
        &mut self,
        task: &promptforge_types::ids::TaskId,
    ) -> Option<crate::execute::scheduler::test_hooks::TaskState> {
        self.scheduler_for_test().task_state_for_test(task)
    }

    /// Shrinks the scheduler's chain-count bound.
    #[cfg(test)]
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.scheduler_for_test().set_max_chains_for_test(limit);
    }

    /// The number of leaf effects the run has issued so far.
    #[cfg(test)]
    pub(crate) fn leaf_requests_issued(&mut self) -> u64 {
        self.scheduler_for_test().leaf_requests_issued()
    }

    /// The run itself, for a test that answers an effect by hand.
    #[cfg(test)]
    pub(crate) fn run_for_test(&mut self) -> &mut Run {
        &mut self.run
    }

    /// The driver's cancel flag, for a test that cancels from another
    /// task.
    #[cfg(test)]
    pub(crate) fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
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
/// mid-run - a host tearing the run down without driving it to its end.
/// Dropping a bare `JoinHandle` detaches the task, which would strand a
/// broker wait or gateway round forever, so the drop applies the same
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
