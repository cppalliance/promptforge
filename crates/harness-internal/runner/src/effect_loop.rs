//! The effect loop: the Harness steps an Engine `Run` and performs its effects.
//!
//! The loop is `step -> record -> perform -> await an answer -> record ->
//! resume`. Every step's events are handed to the run's recorder before any
//! of the step's effects is issued, so an effect's record never precedes the
//! events its step reported. Each effect is appended as its
//! [`EffectRecord`](promptforge::effect::EffectRecord) and then handled by
//! kind. A chat, tool-call, or timer effect is started as a plain task
//! through the tagged spawn wrapper; each task posts
//! `(EffectId, EffectAnswer)` on one channel, and the loop appends the
//! answer's record and resumes the run with it, then steps again. A Vfs
//! effect starts no task: the VFS is synchronous by design, so the loop
//! performs the operation on its own thread, appends the answer's record,
//! and resumes the run at once. It resumes every Vfs effect of a step this
//! way and then steps once, so the loop awaits an answer only when it
//! answered nothing inline. Real-disk operations therefore block the
//! thread that runs the loop, and Vfs operations from different chains run
//! one at a time in issue order.
//!
//! Cancellation is the caller's synchronous flag, awaited beside the
//! answer channel. When it fires the loop cancels the run, aborts every
//! performer still out and joins it, so whatever the performer held is
//! gone before the run ends, then answers each of those effects `Dropped`
//! and steps the run to `Done`. A Vfs effect is never out at a cancel: it
//! is answered before the loop looks at the flag again. A drop is an
//! answer, recorded like any other, so every effect record has exactly one
//! answer record.
//!
//! A performer that panics posts nothing itself; tokio catches the panic
//! and ends the task. Every performer task therefore holds an
//! `Answering` guard that posts `Dropped` for its effect when the task
//! ends without having answered, so the loop hears from every performer
//! it started and a lost one can never leave the run waiting forever. A
//! Vfs operation that panics is caught where it runs and answered
//! `Dropped` the same way.
//!
//! The run's `Done` ends the run with its outcome at the recorder.

use std::collections::HashMap;
use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::effect::{Effect, EffectAnswer, EffectId};
use promptforge::event::{Event, ReplyOrigin};
use promptforge::ids::Provenance;
use promptforge::vfs::{Access, VfsOp};
use promptforge::{Run, RunError, RunResult, Step};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::display_chain::display_chain;
use crate::performers::Performers;
use crate::recorder::{Record, RecordKind, RecorderError, RunId, RunOutcome, RunRecorder};
use crate::spawn::spawn_tagged;

#[path = "effect_loop-answering.rs"]
mod answering;

use answering::{AnswerSender, Answering, answer_vfs};

/// Why the loop stopped without an outcome.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriveError {
    /// The recorder refused a write; the run cannot be recorded, so it is
    /// not driven further.
    #[error(transparent)]
    Recorder(#[from] RecorderError),
    /// The run is pending with nothing issued and nothing out: the run
    /// reports a stall itself, so reaching this means the loop lost a
    /// performer.
    #[error("the effect loop has nothing to await for a pending run")]
    Stalled,
}

/// Drives `run` to its end on the current tokio runtime: performs its
/// effects through `performers`, hands every event, effect, and answer to
/// `recorder` under `run_id`, hands every event to `sink` once it is
/// recorded, and cancels the run when `cancel` fires. Ends the run at the
/// recorder with its outcome and returns it.
///
/// The future is boxed internally: the step machinery is large, and the
/// caller's own future stays small. It is `Send` when `sink` is, so the Harness
/// can hold it in a task of its own; the driver never borrows itself
/// shared across an await.
///
/// # Errors
/// Returns [`DriveError::Recorder`] when the recorder refuses a write and
/// [`DriveError::Stalled`] when the run pends with nothing to await. In
/// either case every performer still out is aborted, the run is
/// abandoned mid-flight, and the recorder is not told it ended.
pub async fn drive_run(
    run: Run,
    performers: Performers,
    recorder: Arc<dyn RunRecorder>,
    run_id: RunId,
    cancel: CancelHandle,
    sink: impl FnMut(Event) + Send,
) -> Result<RunOutcome, DriveError> {
    let mut driver = Driver::new(run, performers, recorder, run_id, cancel, Box::new(sink));
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
    recorder: Arc<dyn RunRecorder>,
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
        recorder: Arc<dyn RunRecorder>,
        run_id: RunId,
        cancel: CancelHandle,
        sink: Box<dyn FnMut(Event) + Send + 'a>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            run,
            performers,
            recorder,
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
                    self.recorder.end_run(self.run_id, outcome.clone()).await?;
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
                    let mut answered_inline = false;
                    for (id, provenance, effect) in effects {
                        self.commit_effect(id, &provenance, &effect).await?;
                        // A Vfs effect comes back from `perform` and is
                        // answered here; every other kind is out as a
                        // task. The run is not stepped between inline
                        // answers: the whole batch is resumed, then
                        // stepped once.
                        if let Some((access, op)) = self.perform(id, &provenance, effect) {
                            let answer = answer_vfs(id, &provenance, access, op);
                            self.commit_answer(id, &provenance, &answer).await?;
                            self.run.resume(id, answer);
                            answered_inline = true;
                        }
                    }
                    if answered_inline {
                        // Stepping again, not awaiting: the answers just
                        // resumed may have made chains ready, and a step
                        // whose every effect was a Vfs effect has nothing
                        // out to await.
                        continue;
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
    async fn await_answer(&mut self) -> Result<(), RecorderError> {
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
    async fn deliver(&mut self, id: EffectId, answer: EffectAnswer) -> Result<(), RecorderError> {
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
    /// answers each of their effects `Dropped`. The join waits for an
    /// aborted task to finish tearing down, so whatever the performer
    /// held is gone before the effect is answered. Only chat, tool-call,
    /// and timer performers are ever out: a Vfs effect is answered inline
    /// and has none.
    async fn drop_outstanding(&mut self) -> Result<(), RecorderError> {
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
                // Harness's bug and is not swallowed.
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
    async fn drop_effect(
        &mut self,
        id: EffectId,
        provenance: &Provenance,
    ) -> Result<(), RecorderError> {
        let answer = EffectAnswer::Dropped;
        self.commit_answer(id, provenance, &answer).await?;
        self.run.resume(id, answer);
        Ok(())
    }

    /// Starts the performer of a chat, tool-call, or timer effect, which
    /// posts the effect's answer under `id`, and returns `None`.
    ///
    /// A Vfs effect starts nothing and is handed back as the access and
    /// operation it carries, for the caller to answer inline. It never
    /// gets an [`Answering`] guard: a guard dropped without posting sends
    /// `Dropped`, and the inline answer is the effect's only one.
    fn perform(
        &mut self,
        id: EffectId,
        provenance: &Provenance,
        effect: Effect,
    ) -> Option<(Arc<Access>, VfsOp)> {
        let tag = (id, provenance.clone());
        let handle = match effect {
            Effect::Chat {
                binding,
                messages,
                tools,
                options,
                round,
            } => {
                let answer = Answering::new(self.tx.clone(), id);
                let on_delta =
                    (round.origin == ReplyOrigin::Chat).then(|| self.performers.on_delta.clone());
                let round = self
                    .performers
                    .broker
                    .chat(binding, messages, tools, options, on_delta);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::Chat(round.await));
                })
            }
            Effect::ToolCall {
                tool, alias, args, ..
            } => {
                let answer = Answering::new(self.tx.clone(), id);
                let call = self.performers.tool.call(tool, alias, args);
                spawn_tagged(tag, async move {
                    answer.post(EffectAnswer::ToolCall(call.await));
                })
            }
            Effect::Vfs { access, op } => return Some((access, op)),
            Effect::Timer { seconds } => {
                let answer = Answering::new(self.tx.clone(), id);
                let sleep = self.performers.timer.sleep(seconds);
                spawn_tagged(tag, async move {
                    sleep.await;
                    answer.post(EffectAnswer::Timer);
                })
            }
        };
        self.outstanding.insert(
            id,
            InFlight {
                provenance: provenance.clone(),
                handle,
            },
        );
        None
    }

    /// Appends one step's events to the recorder, then hands each to the
    /// sink once it is recorded.
    async fn commit_events(&mut self, events: Vec<Event>) -> Result<(), RecorderError> {
        for event in events {
            let record = record(
                event.provenance(),
                RecordKind::Event,
                None,
                serde_json::to_value(&event).map_err(RecorderError::new)?,
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
    ) -> Result<(), RecorderError> {
        let payload = serde_json::to_value(effect.record()).map_err(RecorderError::new)?;
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
    ) -> Result<(), RecorderError> {
        let payload = serde_json::to_value(answer.record()).map_err(RecorderError::new)?;
        self.append(record(
            provenance,
            RecordKind::Answer,
            Some(id.get()),
            payload,
        ))
        .await
    }

    async fn append(&mut self, record: Record) -> Result<(), RecorderError> {
        self.recorder.append(self.run_id, record).await
    }
}

/// Aborts every performer still out when the driver is dropped mid-run -
/// a recorder failure, or the Harness tearing the loop down. Dropping a bare
/// `JoinHandle` detaches the task, which would strand a tool call waiting
/// on the operator or a model round forever, so the drop applies the same
/// abort the run's end does.
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

/// The recorded outcome for the run's result.
fn outcome_of(result: RunResult) -> RunOutcome {
    match result {
        RunResult::Ok(final_text) => RunOutcome::Completed { final_text },
        RunResult::Cancelled => RunOutcome::Cancelled,
        RunResult::Failure(error) => failed_outcome(&error),
    }
}

/// The failed outcome for an Engine error: the failure's kind is the
/// error kind's debug name and its message the error's text with its
/// cause chain. The one derivation for a run that failed under the loop
/// and a run preparation refused, so the two agree in the record.
pub(crate) fn failed_outcome(error: &RunError) -> RunOutcome {
    RunOutcome::Failed {
        kind: format!("{:?}", error.kind()),
        message: display_chain(error),
    }
}
