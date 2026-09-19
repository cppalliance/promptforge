//! The engine's test drivers: hosts for a [`Run`] that need no runtime and
//! no HTTP, for this crate's own suites and, under the `test-support`
//! feature, for companion crates'.
//!
//! [`drive`] is the serial sans-IO driver: it steps a run on the calling
//! thread and answers every effect the moment it is issued, through a
//! closure the caller supplies. Nothing here awaits, spawns, or sleeps; a
//! timer effect is answered however the closure sees fit, so a test's
//! timeouts take no wall time.

#[cfg(test)]
pub(crate) use promptforge_parser::test_support::synthetic_section;

use promptforge_api_types::event::Event;

use crate::Error;
use crate::execute::{
    Effect, EffectAnswer, EffectId, Run, RunError, RunResult, Step, task_history,
};

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
/// use promptforge_api_runtime::test_support::drive;
/// use promptforge_api_runtime::{Prompt, Run, RunContext, RunResult};
/// use promptforge_api_types::event::Event;
/// use promptforge_api_types::observe::NullObserver;
/// use promptforge_api_types::timestamp::Timestamp;
///
/// let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n## Only\n\n```lua\nreturn 'hello'\n```\n";
/// let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
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
