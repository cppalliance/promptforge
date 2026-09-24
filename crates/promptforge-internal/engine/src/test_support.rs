//! The engine's test drivers: hosts for a [`Run`] for this crate's own
//! suites and, under the `test-support` feature, for companion crates'.
//!
//! [`drive`] is the serial sans-IO driver: it steps a run on the calling
//! thread and answers every effect the moment it is issued, through a
//! closure the caller supplies. Nothing there awaits, spawns, or sleeps; a
//! timer effect is answered however the closure sees fit, so a test's
//! timeouts are instant.
//!
//! [`drive_tokio`] is the tokio driver: it performs a run's `Chat`,
//! `ToolCall`, and `UserInput` effects through the caller's [`Performers`]
//! (a struct of boxed async closures, one per kind) on the current tokio
//! runtime, runs store operations on the blocking pool, sleeps timers on
//! the timer wheel, and hands every event to the caller's sink. It is the
//! interim host of Workshop's agent sessions until the harness lands, and
//! the host the engine's own suites drive.
//!
//! [`RunHost`] bundles the resources the suites used to hand the retired
//! in-crate loop - an observer, a client, a fixture tool table, a broker, a
//! delta hook - and [`run_with_host`] is that loop's implicit-prepare path
//! over the tokio driver: prepare, refuse or run. The tool and broker
//! fixtures implement the stand-in traits [`TestTool`] and [`TestBroker`];
//! the production traits are the harness's, which no engine crate names.
//! [`Observer`](recording::Observer) and
//! [`Observation`](recording::Observation) are the suites' recording
//! vocabulary, and [`forward`] is the adapter that replays returned events
//! onto one, so the observation suites hold without rewriting their
//! assertions. Everything else here - the raw-body capture, the null
//! observer, the detail constants, the fixture tool table - is
//! crate-internal test plumbing, not host API.

use std::sync::Arc;

use promptforge_types::event::Event;

use crate::Error;
use crate::execute::{
    Effect, EffectAnswer, EffectId, Environment, Run, RunContext, RunError, RunResult, Step,
    task_history,
};
use crate::parser::Prompt;

pub(crate) mod host;
#[cfg(test)]
#[path = "test_support/mock-gateway-client.rs"]
pub(crate) mod mock_gateway_client;
pub mod recording;
pub(crate) mod tokio_driver;
pub(crate) mod tools;

pub use host::{ChatClient, DeltaHook, RunHost};
pub use recording::forward;
pub use tokio_driver::{BoxFuture, Performer, Performers, drive_tokio};
pub use tools::{TestBroker, TestTool, TestToolTable};

/// The suites' mock-gateway client performs a `Chat` round over its
/// dev-only HTTP under the run's limits.
#[cfg(test)]
impl ChatClient for mock_gateway_client::MockGatewayClient {
    fn complete(
        &self,
        messages: Vec<crate::model::Message>,
        tools: Vec<crate::model::ToolSchema>,
        options: crate::model::CompletionOptions,
        limits: crate::execute::RunLimits,
        on_delta: Option<DeltaHook>,
    ) -> BoxFuture<Result<crate::model::Completion, crate::model::CompletionError>> {
        let client = self.clone();
        Box::pin(async move {
            client
                .complete(
                    &messages,
                    &tools,
                    &options,
                    limits.timeout(),
                    limits.response_bytes(),
                    |delta| {
                        if let Some(hook) = &on_delta {
                            hook(delta);
                        }
                    },
                )
                .await
        })
    }
}

/// Drives `run` to its end on the calling thread, performing every effect
/// through `perform` as it is issued, and returns the run's result with
/// every event it reported, in order.
///
/// The driver is the simplest correct host. After each `step` it answers
/// the step's effects in issue order - each through `perform`, except a
/// [`Effect::TaskEvents`] read, which it answers from the events it has
/// collected so far (the step's own events are collected before its
/// effects are answered, so a task reading its history sees everything
/// reported before the read) - and steps again. Once the run has decided
/// its outcome ([`Run::decided`]), the effects it still issues are
/// answered [`EffectAnswer::Dropped`] without reaching `perform`, as a
/// host abandoning a cancelled run would answer them.
///
/// `perform` is handed the effect's id beside the effect so a scripted
/// performer can correlate answers however it likes; it must return an
/// answer of the effect's own kind (or `Dropped`), as the run requires.
///
/// # Examples
/// A prompt whose only section returns a literal issues no effect, so the
/// performer is never called:
/// ```
/// use std::sync::Arc;
///
/// use promptforge_engine::test_support::drive;
/// use promptforge_engine::{Prompt, Run, RunContext, RunResult};
/// use promptforge_types::event::Event;
/// use promptforge_types::timestamp::Timestamp;
///
/// let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n## Only\n\n```lua\nreturn 'hello'\n```\n";
/// let (prompt, _parse_events) = Prompt::parse(source, "doc-example");
/// let prompt = prompt?;
/// let ctx = RunContext::new("doc-example", 1, Timestamp::UNIX_EPOCH);
/// let run = Run::new(Arc::new(prompt), "", ctx);
/// let (result, events) = drive(run, |_, effect| panic!("no effect is issued: {effect:?}"));
/// let RunResult::Ok(text) = result else {
///     panic!("the literal run succeeds: {result:?}");
/// };
/// assert_eq!(text, "hello");
/// assert!(matches!(events.first(), Some(Event::RunStarted { .. })));
/// assert!(matches!(events.last(), Some(Event::RunSucceeded { .. })));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn drive(
    mut run: Run,
    mut perform: impl FnMut(EffectId, &Effect) -> EffectAnswer,
) -> (RunResult, Vec<Event>) {
    let mut history = Vec::new();
    loop {
        match run.step() {
            Step::Done { result, events } => {
                history.extend(events);
                return (result, history);
            }
            Step::Pending { effects, events } => {
                history.extend(events);
                if effects.is_empty() {
                    // Every effect is answered the step it is issued, so a
                    // pending step that issued nothing has nothing to wait
                    // on: an invariant failure reported rather than a hang.
                    let error = Error::internal(
                        "the serial driver was handed a pending run with no effect to answer",
                    );
                    return (RunResult::Failure(RunError::from(error)), history);
                }
                let decided = run.decided();
                for (id, _, effect) in effects {
                    let answer = if decided {
                        EffectAnswer::Dropped
                    } else if let Effect::TaskEvents { task, last } = &effect {
                        EffectAnswer::TaskEvents(task_history(&history, task, *last))
                    } else {
                        perform(id, &effect)
                    };
                    run.resume(id, answer);
                }
            }
        }
    }
}

/// The retired loop's implicit-prepare path over the tokio driver: prepares
/// and runs `prompt` with the resources `host` bundles.
///
/// The environment's catalog is what prepare fills slots against; a suite
/// with fixture tools installs their descriptors there
/// ([`Environment::tools`] over [`TestToolTable::catalog`]) and the
/// implementations on the host ([`RunHost::tools`]). Capability activation
/// is the harness's and never happens here.
///
/// An unsatisfiable prompt - a missing required capability or an unmet
/// model requirement - is refused with [`RunResult::Failure`] holding
/// [`RequirementsUnmet`](crate::RunErrorKind::RequirementsUnmet) and the
/// model-readable notice naming each gap once.
pub async fn run_with_host(
    env: &Environment,
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
    host: RunHost,
) -> RunResult {
    let (ctx, requirements) = env.prepare(prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return RunResult::Failure(refusal);
    }
    run_host(prompt, args, ctx, host).await
}

/// Runs an already-prepared `prompt` under `ctx` with the resources
/// `host` bundles, on the tokio driver: the host's
/// [`performers`](RunHost::performers) perform the effects under the
/// context's limits, and every event is replayed onto its observer and
/// capture.
pub async fn run_host(prompt: &Prompt, args: &str, ctx: RunContext, host: RunHost) -> RunResult {
    let limits = ctx.limits;
    let run = Run::new(Arc::new(prompt.clone()), args, ctx);
    let cancel = run.cancel_handle();
    drive_tokio(run, host.performers(limits), host.sink(), cancel).await
}
