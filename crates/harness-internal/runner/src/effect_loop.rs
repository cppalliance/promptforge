//! The effect loop: the harness's production host for an engine `Run`.
//!
//! The loop is `step -> record -> perform -> await an answer -> record ->
//! resume`. Every step's events are appended to the run log before any of
//! the step's effects is issued, because a running task may read its own
//! history back through a `TaskEvents` effect and must see everything
//! reported before the read. Each effect is appended as its
//! [`EffectRecord`](promptforge::effect::EffectRecord) and then
//! started through the tagged spawn wrapper: a plain task for the
//! asynchronous kinds, the blocking pool for a store operation (the VFS is
//! synchronous by design). Each task posts `(EffectId, EffectAnswer)` on
//! one channel; the loop appends the answer's record and resumes the run
//! with it, then steps again.
//!
//! Cancellation is the caller's synchronous flag, awaited beside the
//! answer channel. When it fires the loop cancels the run, aborts every
//! performer still out and joins it - a blocking-pool store operation
//! cannot be interrupted, so the join waits for it to finish, and only
//! then is its access clone gone and the claims it held released - then
//! answers each of those effects `Dropped` and steps the run to `Done`. A
//! drop is an answer, recorded like any other, so every effect record in
//! the log has exactly one answer record.
//!
//! A performer that panics posts nothing itself; tokio catches the panic
//! and ends the task. Every performer task therefore holds an
//! `Answering` guard that posts `Dropped` for its effect when the task
//! ends without having answered, so the loop hears from every performer
//! it started and a lost one can never leave the run waiting forever.
//!
//! The run's `Done` closes the run's row in the log with its outcome.

use std::collections::HashMap;
use std::sync::Arc;

use harness_log::{LogError, Record, RecordKind, RunId, RunLog, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::effect::{Effect, EffectAnswer, EffectId};
use promptforge::event::Event;
use promptforge::ids::Provenance;
use promptforge::{Run, RunError, RunResult, Step};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::display_chain::display_chain;
use crate::performers::Performers;
use crate::spawn::{spawn_blocking_tagged, spawn_tagged};

#[path = "effect_loop-answering.rs"]
mod answering;

use answering::{AnswerSender, Answering, perform_store};

/// The run log as the loop and the performers share it: the loop is the
/// writer, a `TaskEvents` performer a reader, and the mutex serializes
/// them. Asynchronous because an append is awaited under it.
pub type SharedLog = Arc<Mutex<RunLog>>;

/// Why the loop stopped without an outcome.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriveError {
    /// The run log refused a write; the run cannot be recorded, so it is
    /// not driven further.
    #[error(transparent)]
    Log(#[from] LogError),
    /// The run is pending with nothing issued and nothing out: the run
    /// reports a stall itself, so reaching this means the loop lost a
    /// performer.
    #[error("the effect loop has nothing to await for a pending run")]
    Stalled,
}

/// Drives `run` to its end on the current tokio runtime: performs its
/// effects through `performers`, records every event, effect, and answer
/// under `run_id` in `log`, hands every event to `sink` once it is
/// recorded, and cancels the run when `cancel` fires. Closes the run's
/// row with its outcome and returns it.
///
/// The future is boxed internally: the step machinery is large, and the
/// caller's own future stays small. It is `Send` when `sink` is, so a host
/// can hold it in a task of its own; the driver never borrows itself
/// shared across an await.
///
/// # Errors
/// Returns [`DriveError::Log`] when the log refuses a write and
/// [`DriveError::Stalled`] when the run pends with nothing to await. In
/// either case every performer still out is aborted, the run is
/// abandoned mid-flight, and its row is left open.
pub async fn drive_run(
    run: Run,
    performers: Performers,
    log: SharedLog,
    run_id: RunId,
    cancel: CancelHandle,
    sink: impl FnMut(Event) + Send,
) -> Result<RunOutcome, DriveError> {
    let mut driver = Driver::new(run, performers, log, run_id, cancel, Box::new(sink));
    Box::pin(driver.drive()).await
}

/// One performer still out: what the loop needs to drop it.
struct InFlight {
    /// The provenance the effect was issued under, for its answer record.
    provenance: Provenance,
    /// The performer's task.
    handle: JoinHandle<()>,
}

/// One run being driven.
struct Driver<'a> {
    run: Run,
    performers: Performers,
    log: SharedLog,
    run_id: RunId,
    sink: Box<dyn FnMut(Event) + Send + 'a>,
    /// Unbounded, because each performer sends exactly once and the
    /// in-flight count is already bounded by the chains that produced the
    /// effects.
    tx: AnswerSender,
    rx: mpsc::UnboundedReceiver<(EffectId, EffectAnswer)>,
    /// The performers still out, keyed by effect. An answer for an id not
    /// here is a late answer for an effect already dropped and is
    /// discarded, so the run never sees two answers for one effect.
    outstanding: HashMap<EffectId, InFlight>,
    cancel: CancelHandle,
}

impl<'a> Driver<'a> {
    fn new(
        run: Run,
        performers: Performers,
        log: SharedLog,
        run_id: RunId,
        cancel: CancelHandle,
        sink: Box<dyn FnMut(Event) + Send + 'a>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            run,
            performers,
            log,
            run_id,
            sink,
            tx,
            rx,
            outstanding: HashMap::new(),
            cancel,
        }
    }

    async fn drive(&mut self) -> Result<RunOutcome, DriveError> {
        loop {
            match self.run.step() {
                Step::Done { result, events } => {
                    self.commit_events(events).await?;
                    // `Done` is returned only once every effect is
                    // answered, so nothing is out.
                    let outcome = outcome_of(result);
                    self.log
                        .lock()
                        .await
                        .end_run(self.run_id, outcome.clone())
                        .await?;
                    return Ok(outcome);
                }
                Step::Pending { effects, events } => {
                    // The run's own word, not a scan of its events: the
                    // events are a report, not a control channel. A cancel
                    // that fired before this step is handed to the run here
                    // so its next step observes it.
                    if self.cancel.is_cancelled() {
                        self.run.cancel();
                    }
                    let decided = self.run.decided() || self.cancel.is_cancelled();
                    self.commit_events(events).await?;
                    if decided {
                        // Every effect issued in this step is moot before
                        // it is performed, and every performer still out
                        // is moot too. Record each effect and answer it
                        // `Dropped` so the next step reaches `Done`.
                        for (id, provenance, effect) in effects {
                            self.commit_effect(id, &provenance, &effect).await?;
                            self.drop_effect(id, &provenance).await?;
                        }
                        self.drop_outstanding().await?;
                        continue;
                    }
                    for (id, provenance, effect) in effects {
                        self.commit_effect(id, &provenance, &effect).await?;
                        self.perform(id, provenance, effect);
                    }
                    if self.outstanding.is_empty() {
                        return Err(DriveError::Stalled);
                    }
                    self.await_answer().await?;
                }
            }
        }
    }

    /// Waits for the next answer, applying every answer already queued
    /// behind it, or acts on the cancel flag as soon as it is set. Both
    /// arms are event-driven: the channel wakes on a posted answer and the
    /// flag's future wakes on the cancel, so a fully suspended run costs
    /// no wakeups while it waits.
    async fn await_answer(&mut self) -> Result<(), LogError> {
        tokio::select! {
            biased;
            arrival = self.rx.recv() => {
                if let Some((id, answer)) = arrival {
                    self.deliver(id, answer).await?;
                }
                while let Ok((id, answer)) = self.rx.try_recv() {
                    self.deliver(id, answer).await?;
                }
            }
            () = self.cancel.cancelled() => {
                self.run.cancel();
                self.drop_outstanding().await?;
            }
        }
        Ok(())
    }

    /// Records one performer's answer and resumes the run with it, unless
    /// the effect was already dropped, in which case the late answer is
    /// discarded.
    async fn deliver(&mut self, id: EffectId, answer: EffectAnswer) -> Result<(), LogError> {
        let Some(in_flight) = self.outstanding.remove(&id) else {
            return Ok(());
        };
        if matches!(answer, EffectAnswer::Dropped) {
            // A performer answers with its kind's payload, never with
            // `Dropped`: only the task's guard posts that, and only when
            // the task ended without answering. The loop aborts a task
            // only after removing its effect from `outstanding`, so a
            // guard's post that reaches here is a performer that
            // panicked.
            tracing::error!(
                effect = %id,
                task = %in_flight.provenance.task,
                "a performer ended without answering; its effect is dropped"
            );
        }
        self.commit_answer(id, &in_flight.provenance, &answer)
            .await?;
        self.run.resume(id, answer);
        Ok(())
    }

    /// Aborts and joins every performer still out, in effect order, and
    /// answers each of their effects `Dropped`. A blocking-pool store
    /// operation cannot be interrupted, so its join waits for it to
    /// finish; only then is its access clone - and the claims it holds -
    /// gone, which is what keeps claim release bounded to the run's
    /// lifetime.
    async fn drop_outstanding(&mut self) -> Result<(), LogError> {
        let mut outstanding: Vec<(EffectId, InFlight)> =
            std::mem::take(&mut self.outstanding).into_iter().collect();
        outstanding.sort_by_key(|(id, _)| id.get());
        for (id, in_flight) in outstanding {
            in_flight.handle.abort();
            match in_flight.handle.await {
                // The performer finished before the abort took, or the
                // abort took: both are the expected ends of a dropped
                // performer, and whatever it posted is discarded below.
                Ok(()) => {}
                Err(join) if join.is_cancelled() => {}
                // A performer that panicked before the drop reached it.
                // Its effect is dropped either way, but the panic is the
                // host's bug and is not swallowed.
                Err(join) => tracing::error!(
                    effect = %id,
                    task = %in_flight.provenance.task,
                    panic = %join,
                    "a performer panicked before its effect was dropped"
                ),
            }
            self.drop_effect(id, &in_flight.provenance).await?;
        }
        // Whatever the joined performers posted before the abort, or
        // their guards posted at the abort, is stale: their effects are
        // answered.
        while self.rx.try_recv().is_ok() {}
        Ok(())
    }

    /// Records the `Dropped` answer for one effect and resumes the run
    /// with it.
    async fn drop_effect(&mut self, id: EffectId, provenance: &Provenance) -> Result<(), LogError> {
        let answer = EffectAnswer::Dropped;
        self.commit_answer(id, provenance, &answer).await?;
        self.run.resume(id, answer);
        Ok(())
    }

    /// Starts one effect's performer, which posts the effect's answer
    /// under `id`.
    fn perform(&mut self, id: EffectId, provenance: Provenance, effect: Effect) {
        let answer = Answering::new(self.tx.clone(), id);
        let tag = (id, provenance.clone());
        let handle = match effect {
            Effect::Chat {
                binding,
                messages,
                tools,
                options,
                stream,
            } => {
                let round = self
                    .performers
                    .chat
                    .chat(binding, messages, tools, options, stream);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::Chat(round.await));
                })
            }
            Effect::ToolCall { tool, alias, args } => {
                let call = self.performers.tool.call(tool, alias, args);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::ToolCall(call.await));
                })
            }
            Effect::UserInput { execution, section } => {
                let wait = self.performers.input.wait(execution, section);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::UserInput(wait.await));
                })
            }
            Effect::Store { access, op } => {
                let store = Arc::clone(&self.performers.store);
                spawn_blocking_tagged(tag, move || {
                    let result = perform_store(store.as_ref(), access, op);
                    answer.post(EffectAnswer::Store(result));
                })
            }
            Effect::Timer { seconds } => {
                let sleep = self.performers.timer.sleep(seconds);
                spawn_tagged(tag, async move {
                    sleep.await;
                    answer.post(EffectAnswer::Timer);
                })
            }
            Effect::TaskEvents { task, last } => {
                let read = self.performers.task_events.events(task, last);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::TaskEvents(read.await));
                })
            }
        };
        self.outstanding.insert(id, InFlight { provenance, handle });
    }

    /// Appends one step's events to the log, then hands each to the sink
    /// once it is recorded.
    async fn commit_events(&mut self, events: Vec<Event>) -> Result<(), LogError> {
        for event in events {
            let record = record(
                event.provenance(),
                RecordKind::Event,
                None,
                serde_json::to_value(&event)?,
            );
            self.append(record).await?;
            (self.sink)(event);
        }
        Ok(())
    }

    /// Appends one issued effect's record.
    async fn commit_effect(
        &mut self,
        id: EffectId,
        provenance: &Provenance,
        effect: &Effect,
    ) -> Result<(), LogError> {
        let payload = serde_json::to_value(effect.record())?;
        self.append(record(
            provenance,
            RecordKind::Effect,
            Some(id.get()),
            payload,
        ))
        .await
    }

    /// Appends one answer's record under its effect's provenance.
    async fn commit_answer(
        &mut self,
        id: EffectId,
        provenance: &Provenance,
        answer: &EffectAnswer,
    ) -> Result<(), LogError> {
        let payload = serde_json::to_value(answer.record())?;
        self.append(record(
            provenance,
            RecordKind::Answer,
            Some(id.get()),
            payload,
        ))
        .await
    }

    async fn append(&mut self, record: Record) -> Result<(), LogError> {
        self.log
            .lock()
            .await
            .append(self.run_id, record)
            .await
            .map(|_seq| ())
    }
}

/// Aborts every performer still out when the driver is dropped mid-run -
/// a log failure, or a host tearing the loop down. Dropping a bare
/// `JoinHandle` detaches the task, which would strand an input wait or a
/// model round forever, so the drop applies the same abort the run's end
/// does.
impl Drop for Driver<'_> {
    fn drop(&mut self) {
        for in_flight in self.outstanding.values() {
            in_flight.handle.abort();
        }
    }
}

/// One record under `provenance`.
fn record(
    provenance: &Provenance,
    kind: RecordKind,
    effect_id: Option<u64>,
    payload: serde_json::Value,
) -> Record {
    Record {
        task_id: provenance.task.to_string(),
        task_seq: provenance.seq,
        kind,
        effect_id,
        payload,
    }
}

/// The log's outcome for the run's result.
fn outcome_of(result: RunResult) -> RunOutcome {
    match result {
        RunResult::Ok(final_text) => RunOutcome::Completed { final_text },
        RunResult::Cancelled => RunOutcome::Cancelled,
        RunResult::Failure(error) => failed_outcome(&error),
    }
}

/// The log's failed outcome for an engine error: `runs.error_kind` is the
/// kind's debug name and `runs.error_message` the error's text with its
/// cause chain. The one derivation for a run that failed under the loop
/// and a run preparation refused, so the two agree in the log.
pub(crate) fn failed_outcome(error: &RunError) -> RunOutcome {
    RunOutcome::Failed {
        kind: format!("{:?}", error.kind()),
        message: display_chain(error),
    }
}
