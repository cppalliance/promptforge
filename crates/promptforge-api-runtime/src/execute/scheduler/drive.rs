//! The driver loop: `resume -> match request -> dispatch -> resume with
//! answer`, run to the chain arena's terminal state. The loop drains the
//! ready queue, then awaits the answer channel or cancellation, whichever
//! comes first; an empty ready queue with an empty pending table is a
//! stall, which fails loudly rather than hangs. The run's result is
//! withheld until every in-flight leaf task has been aborted and joined,
//! so no store op's access clone outlives the run.

use crate::execute::protocol::Answer;
use crate::execute::support::GENERIC_COMPLETION;
use crate::{Error, Result, cancel};

use super::{Arrival, Scheduler};

/// Aborts every in-flight leaf task when the driver future is dropped
/// mid-suspension - a host tearing the run down without polling it to a
/// terminal state. Dropping a bare `JoinHandle` detaches the task, which
/// would strand a broker wait or gateway round forever (a session close
/// would leak its pending input wait and never emit `input_cancelled`),
/// so the drop path applies the same abort the cancellation path does.
/// The claims-release join in [`Scheduler::drain_io_tasks`] is unnecessary
/// here: a dropped run delivers no result.
impl Drop for Scheduler<'_> {
    fn drop(&mut self) {
        for handle in self.io_tasks.values() {
            handle.abort();
        }
    }
}

impl Scheduler<'_> {
    /// Drives the run until it ends and returns the run's result: the H1
    /// pass first when the prompt has H1 blocks, then the root chain over
    /// the prompt's sections.
    ///
    /// Leaf dispatch spawns plain tasks (not `spawn_local`): an infer task
    /// touches no scheduler state and no Lua value - it awaits one gateway
    /// round and posts the answer to the channel - so the driver future
    /// stays `Send` and a caller may spawn the run onto a multi-thread
    /// runtime. On a current-thread runtime the spawned tasks run on that
    /// one thread anyway.
    ///
    /// # Errors
    /// Returns the [`Error`] of whichever step failed: frame construction,
    /// a Lua block, or a dispatched request's answer.
    /// Returns [`Error::Interrupted`] when the run's cancellation handle is
    /// signaled while chains are running or suspended.
    pub(crate) async fn drive(&mut self) -> Result<String> {
        let result = self.drive_inner().await;
        self.drain_io_tasks().await;
        result
    }

    /// Claims-release ordering constraint: the run's result - success,
    /// determinism failure, or cancellation alike - must not be delivered
    /// while an in-flight leaf op still holds its access clone. A store
    /// op runs on the blocking pool, where aborting the task detaches
    /// rather than interrupts, so an abandoned op would release its
    /// identity's claims only when the closure finishes - past the run's
    /// end, where a fresh access could meet the lingering claim. Abort
    /// every task still recorded (prompt for an async task, a no-op for
    /// a blocking op already running, which runs to completion), then
    /// await each handle: the join resolves only once the op's access
    /// clone - and with it the identity's claims - is gone. This changes
    /// when claims release, never what an operation does.
    async fn drain_io_tasks(&mut self) {
        let tasks = std::mem::take(&mut self.io_tasks);
        for task in tasks.values() {
            task.abort();
        }
        for (_, task) in tasks {
            let _ = task.await;
        }
    }

    async fn drive_inner(&mut self) -> Result<String> {
        // The H1 pass runs when the prompt has H1 blocks; an H1-less prompt
        // goes straight to the walk, so its shared library never pays for a
        // throwaway section-0 replay.
        if self.ctx.prompt().h1_blocks().is_empty() {
            let sections = self.ctx.prompt().sections();
            if sections.is_empty() {
                return Ok(GENERIC_COMPLETION.to_owned());
            }
            self.start_root_walk(sections)?;
        } else {
            let h1 = self.start_live_h1()?;
            self.ready.push_back(h1);
        }
        let mut root_result = None;
        loop {
            while let Some(id) = self.ready.pop_front() {
                // Cancellation between steps: the instruction hook covers
                // running Lua and the select below covers suspension, but a
                // run whose chains never suspend on I/O would otherwise
                // finish without ever observing the handle - the legacy
                // fanout driver's select loop observed it at arm
                // boundaries.
                if cancel::is_cancelled() {
                    return Err(Error::Interrupted);
                }
                if let Err(error) = self.step(id, &mut root_result).await {
                    self.finish(id, Err(error), &mut root_result);
                }
                if let Some(result) = root_result.take() {
                    return result;
                }
            }
            // Every unfinished chain is ready, pending on I/O, blocked on a
            // child, or waiting on a task, and a blocked or waiting chain
            // transitively bottoms out in a ready or pending chain, so an
            // empty ready queue with an empty pending table can only be a
            // driver bug (nothing ready, nothing pending, and whatever is
            // waiting can never be woken) - fail loudly rather than hang.
            if self.pending.is_empty() {
                return Err(Error::internal(
                    "the scheduler stalled with no ready chain and no in-flight request",
                ));
            }
            tokio::select! {
                biased;
                // Cancellation while suspended: abort the in-flight leaf
                // tasks and fail the run. The suspended chains' frames drop
                // unarmed with the scheduler - the same outcome as the
                // hook-driven path while running - and each fanout arm's
                // finalizer drop reports its FANOUT_ARM_CANCELLED terminal
                // observation, so the exactly-once terminal contract holds
                // on this path too.
                () = cancel::wait_cancelled() => {
                    for handle in self.io_tasks.values() {
                        handle.abort();
                    }
                    return Err(Error::Interrupted);
                }
                arrival = self.answers.recv() => {
                    let Some((request_id, arrival)) = arrival else {
                        return Err(Error::internal(
                            "the answer channel cannot close while the scheduler holds its sender",
                        ));
                    };
                    self.io_tasks.remove(&request_id);
                    let Some(parked) = self.pending.remove(&request_id) else {
                        // A late answer from an I/O task whose chain was
                        // already aborted (a fatal sibling's fanout abort
                        // races a task that sent before the abort landed):
                        // the abort recorded the request id, so the answer
                        // is moot. Any other unknown id means the driver
                        // dropped a pending entry early - answer loss that
                        // must fail loudly, not pass silently.
                        if self.aborted_requests.remove(&request_id) {
                            continue;
                        }
                        return Err(Error::internal(
                            "an answer arrived for a request with no pending entry and no recorded abort",
                        ));
                    };
                    // A chat round's completion becomes its answer here, on
                    // the driver thread: the round's events fire against the
                    // chain's own reporting handles and the tool calls are
                    // checked against the scope the chain advertised.
                    let answer = match arrival {
                        Arrival::Answer(answer) => answer,
                        Arrival::Chat(result) => self.accept_chat(parked, result)?,
                    };
                    match answer {
                        // A claims-model conflict is fatal: the run ends on
                        // the spot with the determinism violation rather
                        // than resuming it into Lua, where an author
                        // `pcall` could catch it. The suspended chains drop
                        // unarmed with the scheduler, each fanout arm's
                        // finalizer reporting its cancelled terminal
                        // observation, exactly as on the cancellation path.
                        Answer::Store(Err(error @ Error::Determinism(_))) => return Err(error),
                        answer => {
                            self.chains[parked.index()].incoming = Some(answer);
                            self.ready.push_back(parked);
                        }
                    }
                }
            }
        }
    }
}
