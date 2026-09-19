//! The in-crate tokio host: a loop over [`Run`] that performs its effects
//! with the resources a [`RunHost`] bundles - the gateway client, the tool
//! implementation table, the input broker, the blocking pool for store
//! operations, and the tokio timer wheel - and forwards its events to the
//! host's observer and capture.
//!
//! The loop is `step -> perform -> await an answer -> resume`. Every
//! effect the step hands out is spawned as one performer that posts its
//! answer on a channel under the effect's id; the loop resumes the run
//! with each arriving answer and steps again. A `TaskEvents` read is
//! answered at issue from the driver's own history of forwarded events.
//! When the run reports itself decided ([`Run::decided`]) every performer
//! still out is aborted and joined - a blocking-pool store operation runs
//! to completion, so its access clone and the claims it holds release
//! before the result is delivered - and its effect is answered `Dropped`,
//! as is every effect issued in the deciding step itself, which is never
//! performed; so the run reaches `Done` with every effect answered exactly
//! once.
//!
//! Cancellation is the run's synchronous flag: the host sets it (through
//! the context's handle or [`Run::cancel`]), running Lua observes it from
//! its instruction hook, and this loop awaits the flag beside the answer
//! channel so a run whose chains are all suspended tears down promptly.
//!
//! This is the engine's only in-process host until the harness replaces
//! it; the suites drive it in place of the scheduler they used to drive.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use promptforge_api_types::event::Event;
use promptforge_api_types::tools::ToolError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cancel::CancelHandle;
use crate::client::{CompletionError, GatewayClient};
use crate::input::InputOutcome;
use crate::lua::run_store_op;
use crate::store::Store;
use crate::{Error, Result};

use super::RunResult;
#[cfg(test)]
use super::context::RunState;
use super::gateway::GatewaySource;
use super::host::RunHost;
#[cfg(test)]
use super::run::EffectRecord;
use super::run::{Effect, EffectAnswer, EffectId, Run, Step};
#[cfg(test)]
use super::scheduler::Scheduler;

/// The send half every performer posts its answer to.
type AnswerSender = mpsc::UnboundedSender<(EffectId, EffectAnswer)>;

/// One run driven on tokio.
pub(crate) struct TokioDriver {
    /// The run being driven.
    run: Run,
    /// The host resources the performers draw on (the tool table, the
    /// input broker, the delta hook) and the observer and capture the
    /// events are forwarded to.
    host: RunHost,
    /// The run's gateway source: the client the host supplied, or the
    /// environment with the run's HTTP limits, resolved on the first
    /// `Chat` effect so a construction error surfaces as that round's
    /// failure rather than being swallowed.
    gateway: GatewaySource,
    /// The resolved client, cached for the run.
    client: Option<GatewayClient>,
    /// The answer channel: unbounded, because each performer sends exactly
    /// once and the in-flight count is already bounded by the chains that
    /// produced the effects.
    tx: AnswerSender,
    rx: mpsc::UnboundedReceiver<(EffectId, EffectAnswer)>,
    /// The performers still out, keyed by effect. An answer for an id not
    /// here is a late answer for an effect already dropped and is
    /// discarded, so the run never sees two answers for one effect.
    outstanding: HashMap<EffectId, JoinHandle<()>>,
    /// The run's cancel flag, awaited while the loop waits on answers.
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

impl TokioDriver {
    /// Builds the driver for one run over `state`: the suites' entry, which
    /// shape the context themselves. The host is the one the suite set on
    /// its context (observer, broker, tools, delta hook), with `client` as
    /// the run's gateway client when the suite supplies one.
    #[cfg(test)]
    pub(crate) fn new(state: &RunState, client: Option<GatewayClient>) -> Self {
        let mut host = state.test_host();
        if let Some(client) = client {
            host = host.client(client);
        }
        Self::over(Run::from_state(state.clone()), host)
    }

    /// Builds the driver over an assembled run with the host's resources.
    /// A client the host supplies is used as given; otherwise one is built
    /// from the environment on the first `Chat`, honoring the run's HTTP
    /// limits.
    pub(crate) fn over(run: Run, host: RunHost) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let limits = run
            .state()
            .map(super::context::RunState::limits)
            .unwrap_or_default();
        let client = host
            .client
            .clone()
            .map(|client| client.with_request_limits(limits.timeout(), limits.response_bytes()));
        Self {
            cancel: run.cancel_handle(),
            run,
            host,
            gateway: GatewaySource::from_optional(client, limits),
            client: None,
            tx,
            rx,
            outstanding: HashMap::new(),
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
                    // events are a report, and control never rides on them.
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
    /// behind it, or returns as soon as the cancel flag is set so the
    /// run's next step observes it. Both arms are event-driven: the
    /// channel wakes on a posted answer and the flag's future wakes on
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
            // The flag itself is the run's to act on at its next step;
            // the wake only makes that step happen promptly.
            () = self.cancel.cancelled() => {}
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

    /// Forwards one step's events to the host's observer and capture, and
    /// appends them to the history `TaskEvents` reads answer from.
    fn forward(&mut self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        super::events_to_observer::forward(
            events.clone(),
            self.host.observer.as_ref(),
            self.host.debug.as_deref(),
        );
        self.history.extend(events);
    }

    /// The run's gateway client, resolved on first use and cached.
    ///
    /// # Errors
    /// Returns the client's construction error when the environment
    /// cannot build one.
    fn client(&mut self) -> std::result::Result<GatewayClient, CompletionError> {
        if let Some(client) = &self.client {
            return Ok(client.clone());
        }
        let client = self.gateway.resolve()?;
        self.client = Some(client.clone());
        Ok(client)
    }

    /// Performs one effect: spawns the performer that will post the
    /// effect's answer under `id` and returns `true`, or answers at once
    /// and returns `false` when the effect cannot be performed (no client
    /// can be built for a `Chat`, the tool a `ToolCall` names is not in
    /// the host's table, no broker serves a `UserInput`).
    fn perform(&mut self, id: EffectId, effect: Effect) -> bool {
        let tx = self.tx.clone();
        let handle = match effect {
            Effect::Chat {
                messages,
                tools,
                options,
                stream,
                ..
            } => {
                let client = match self.client() {
                    Ok(client) => client,
                    Err(error) => {
                        self.run.resume(id, EffectAnswer::Chat(Err(error)));
                        return false;
                    }
                };
                // The host's delta callback is the live consumer of a
                // streaming round; without one, or for a round the effect
                // marks non-streaming (a nested infer), the chunks drop at
                // the leaf and the completed reply is the repair.
                let on_delta = stream.then(|| self.host.on_delta.clone()).flatten();
                tokio::spawn(async move {
                    let tool_arg = (!tools.is_empty()).then_some(tools.as_slice());
                    let result = client
                        .complete(&messages, tool_arg, &options, |delta| {
                            if let Some(hook) = &on_delta {
                                hook(delta);
                            }
                        })
                        .await
                        .map(Box::new);
                    post(&tx, id, EffectAnswer::Chat(result));
                })
            }
            Effect::ToolCall { tool, args, .. } => {
                // Resolved by the stable identity against the host's
                // implementation table, as a harness resolves it against
                // its activated capabilities; the alias is the record's,
                // not the resolver's.
                let Some(tool) = self.host.tools.get(&tool) else {
                    self.run.resume(
                        id,
                        EffectAnswer::ToolCall(Err(ToolError::message(
                            "the tool the call names has no implementation in the host's table",
                        ))),
                    );
                    return false;
                };
                tokio::spawn(async move {
                    let result = tool.call(args).await;
                    post(&tx, id, EffectAnswer::ToolCall(result));
                })
            }
            Effect::UserInput { execution, section } => {
                let Some(broker) = self.host.input.clone() else {
                    self.run
                        .resume(id, EffectAnswer::UserInput(Ok(InputOutcome::Unavailable)));
                    return false;
                };
                tokio::spawn(async move {
                    let result = broker.user_input(&execution, &section).await;
                    post(&tx, id, EffectAnswer::UserInput(result));
                })
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
                let events = super::task_history(&self.history, &task, last);
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
    fn record(&self, effects: &[(EffectId, promptforge_api_types::ids::Provenance, Effect)]) {
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
        task: &promptforge_api_types::ids::TaskId,
    ) -> Option<super::scheduler::TaskState> {
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

    /// The run's cancel flag, for a test that cancels from another task.
    #[cfg(test)]
    pub(crate) fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }
}

impl std::fmt::Debug for TokioDriver {
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
impl Drop for TokioDriver {
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
