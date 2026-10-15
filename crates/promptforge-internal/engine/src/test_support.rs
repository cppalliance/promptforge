//! The Engine's test drivers: each steps a [`Run`] and answers its effects
//! in this crate's suites and, under the `test-support` feature, companion
//! crates'.
//!
//! [`drive`] is the serial sans-IO driver: it steps a run on the calling
//! thread and answers every effect the moment it is issued, through a
//! closure the caller supplies. Nothing there awaits, spawns, or sleeps; a
//! timer effect is answered however the closure sees fit, so a test's
//! timeouts are instant. In this crate's own tests, `scenario` builds a
//! whole run from its setup as data and drives it on the serial driver.
//!
//! [`drive_tokio`] is the tokio driver: it performs a run's `Chat` and
//! `ToolCall` effects through the caller's [`Performers`]
//! (a struct of boxed async closures, one per kind) on the current tokio
//! runtime, runs store operations on the blocking pool, sleeps timers on
//! the timer wheel, and hands every event to the caller's sink. It steps
//! the runs in the Engine's own suites.
//!
//! [`RunFixture`] bundles a suite's resources for one run - an observer, a
//! client, a fixture tool table - and [`run_with_fixture`] is
//! the implicit-prepare path over the tokio driver: prepare, refuse or
//! run. The tool fixtures implement
//! the stand-in trait [`TestTool`].
//! [`Observer`](recording::Observer) and
//! [`Observation`](recording::Observation) are the suites' recording
//! vocabulary, and [`forward`] is the adapter that replays returned events
//! onto one, so the observation suites hold without rewriting their
//! assertions. Everything else here - the raw-body capture, the null
//! observer, the detail constants, the fixture tool table - is
//! crate-internal test plumbing, outside the facade API.

use std::sync::Arc;

use promptforge_types::event::Event;

use crate::Error;
use crate::execute::{Effect, EffectAnswer, EffectId, Run, RunContext, RunError, RunResult, Step};
use crate::parser::Prompt;

mod fixture;
pub mod recording;
#[cfg(test)]
pub(crate) mod scenario;
#[cfg(test)]
#[path = "test_support/scripted-chat.rs"]
pub(crate) mod scripted_chat;
#[cfg(test)]
pub(crate) mod seeded;
#[cfg(test)]
pub(crate) mod temp_root;
pub(crate) mod tokio_driver;
mod tools;

pub use fixture::{ChatClient, RunFixture};
pub use recording::forward;
pub use tokio_driver::{BoxFuture, Performer, Performers, drive_tokio};
#[cfg(test)]
pub(crate) use tools::FnTool;
pub use tools::{TestTool, TestToolTable};

/// The suites' scripted model answers a `Chat` round in process under the
/// run's limits.
#[cfg(test)]
impl ChatClient for scripted_chat::ScriptedChat {
    fn complete(
        &self,
        messages: Vec<crate::model::Message>,
        tools: Vec<crate::model::ToolSchema>,
        options: crate::model::CompletionOptions,
        limits: crate::execute::RunLimits,
    ) -> BoxFuture<Result<crate::model::Completion, crate::model::CompletionError>> {
        let chat = self.clone();
        Box::pin(async move {
            chat.complete(
                &messages,
                &tools,
                &options,
                limits.timeout(),
                limits.response_bytes(),
            )
            .await
        })
    }
}

/// Drives `run` to its end on the calling thread, performing every effect
/// through `perform` as it is issued, and returns the run's result with
/// every event it reported, in order.
///
/// The driver is the simplest correct caller of a run. After each `step`
/// it answers the step's effects in issue order, each through `perform`,
/// and steps again. Once the run has decided
/// its outcome ([`Run::decided`]), the effects it still issues are
/// answered [`EffectAnswer::Dropped`] without reaching `perform`, as a
/// caller abandoning a cancelled run would answer them.
///
/// `perform` is handed the effect's id beside the effect so a scripted
/// performer can correlate answers however it likes; it must return an
/// answer of the effect's own kind (or `Dropped`), as the run requires.
pub fn drive(
    mut run: Run,
    mut perform: impl FnMut(EffectId, &Effect) -> EffectAnswer,
) -> (RunResult, Vec<Event>) {
    let mut reported = Vec::new();
    loop {
        if let Some(result) = drive_step(&mut run, &mut perform, &mut |_, _| {}, &mut reported) {
            return (result, reported);
        }
    }
}

/// One step of [`drive`]: steps `run`, appends the step's events to
/// `reported`, answers every effect the step issued, hands each effect and
/// its answer to `answered`, `Dropped` ones included, and returns the run's
/// result once it has one.
pub(crate) fn drive_step(
    run: &mut Run,
    perform: &mut impl FnMut(EffectId, &Effect) -> EffectAnswer,
    answered: &mut impl FnMut(&Effect, &EffectAnswer),
    reported: &mut Vec<Event>,
) -> Option<RunResult> {
    match run.step() {
        Step::Done { result, events } => {
            reported.extend(events);
            Some(result)
        }
        Step::Pending {
            effects, events, ..
        } => {
            reported.extend(events);
            if effects.is_empty() {
                // Every effect is answered the step it is issued, so a
                // pending step that issued nothing has nothing to wait
                // on: an invariant failure reported rather than a hang.
                let error = Error::internal(
                    "the serial driver was handed a pending run with no effect to answer",
                );
                return Some(RunResult::Failure(RunError::from(error)));
            }
            let decided = run.decided();
            for (id, _, effect) in effects {
                let answer = if decided {
                    EffectAnswer::Dropped
                } else {
                    perform(id, &effect)
                };
                answered(&effect, &answer);
                run.resume(id, answer);
            }
            None
        }
    }
}

/// The implicit-prepare path over the tokio driver: prepares and runs
/// `prompt` with the resources `fixture` bundles.
///
/// The context's catalog is what the run offers; a suite with fixture
/// tools sets their descriptors there
/// ([`RunContext::tools`] over [`TestToolTable::catalog`]) and the
/// implementations on `fixture` ([`RunFixture::tools`]). Plugins live
/// outside the Engine, and none is installed here.
///
/// An unsatisfiable prompt - a missing required Plugin or an unmet
/// model requirement - is refused with [`RunResult::Failure`] holding
/// [`RequirementsUnmet`](crate::RunErrorKind::RequirementsUnmet) and the
/// model-readable notice naming each gap once.
pub async fn run_with_fixture(
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
    fixture: RunFixture,
) -> RunResult {
    let (ctx, requirements) = ctx.prepare(prompt);
    if let Some(refusal) = requirements.refusal() {
        return RunResult::Failure(refusal);
    }
    run_prepared(prompt, args, ctx, fixture).await
}

/// Runs an already-prepared `prompt` under `ctx` with the resources
/// `fixture` bundles, on the tokio driver: the bundle's
/// [`performers`](RunFixture::performers) perform the effects under the
/// context's limits, and every event is replayed onto its observer and
/// capture.
pub async fn run_prepared(
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
    fixture: RunFixture,
) -> RunResult {
    let limits = ctx.limits;
    let run = Run::new(Arc::new(prompt.clone()), args, ctx);
    let cancel = run.cancel_handle();
    drive_tokio(run, fixture.performers(limits), fixture.sink(), cancel).await
}
