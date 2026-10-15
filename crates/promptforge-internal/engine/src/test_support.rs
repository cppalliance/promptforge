//! The Engine's test drivers: each steps a [`Run`] and answers its effects,
//! in this crate's suites and, under the `test-support` feature, in its
//! bench.
//!
//! [`drive`] is the serial sans-IO driver: it steps a run on the calling
//! thread and answers every effect the moment it is issued, through a
//! closure the caller supplies. Nothing there awaits, spawns, or sleeps; a
//! timer effect is answered however the closure sees fit, so a test's
//! timeouts are instant. [`perform_locally`] answers every effect in
//! process, a model round through a closure of the caller's, and
//! [`ScriptedChat`](scripted_chat::ScriptedChat) plays that model from a
//! script of replies.
//!
//! The `test-support` feature compiles those three alone, for
//! `benches/models_loop.rs`, its only consumer: a bench is a separate
//! target that cannot see `#[cfg(test)]` code. The rest is this crate's
//! own test plumbing. `scenario` builds a whole run from its setup as data
//! and drives it on the serial driver, and `rig` drives a scenario's run
//! with its effects tapped, its held effects released in the order a test
//! chooses or its answers shuffled under a seed, and its scheduler in
//! reach. `recording` is the suites' observation vocabulary, and its
//! `forward` replays returned events onto an observer, so the observation
//! suites hold without rewriting their assertions. `FnTool` is the
//! fixture tool, a closure answered at once.

use promptforge_types::event::Event;
use promptforge_types::tools::ToolError;

use crate::Error;
use crate::execute::{Effect, EffectAnswer, EffectId, Run, RunError, RunResult, Step};
use crate::lua::run_store_op;

#[cfg(test)]
pub mod recording;
#[cfg(test)]
pub(crate) mod rig;
#[cfg(test)]
pub(crate) mod scenario;
#[path = "test_support/scripted-chat.rs"]
pub mod scripted_chat;
#[cfg(test)]
pub(crate) mod seeded;
#[cfg(test)]
pub(crate) mod temp_root;
#[cfg(test)]
mod tools;

#[cfg(test)]
pub use recording::forward;
#[cfg(test)]
pub(crate) use tools::FnTool;

/// A fresh default handle's access capability, for tests that inject Engine
/// values into a standalone VM.
#[cfg(test)]
#[track_caller]
pub(crate) fn fresh_access() -> std::sync::Arc<promptforge_vfs::Access> {
    std::sync::Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// Performs one effect locally, with no I/O: a store operation runs on the
/// effect's own access handle, a timer fires at once, a bound tool is
/// unbound, and a model round is answered by
/// `chat`, a function of the effect alone so the answer never depends on
/// arrival order.
pub fn perform_locally(
    effect: &Effect,
    chat: &mut impl FnMut(&Effect) -> EffectAnswer,
) -> EffectAnswer {
    match effect {
        Effect::Chat { .. } => chat(effect),
        Effect::ToolCall { alias, .. } => EffectAnswer::ToolCall(Err(ToolError::message(format!(
            "no tool is bound as {alias} in this test"
        )))),
        Effect::Vfs { access, op } => EffectAnswer::Vfs(run_store_op(access, op.clone())),
        Effect::Timer { .. } => EffectAnswer::Timer,
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
    let mut answer = |id: EffectId, effect: &Effect| Some(perform(id, effect));
    loop {
        let stepped = drive_step(
            &mut run,
            &mut |_| {},
            &mut answer,
            &mut |_, _| {},
            &mut Vec::new(),
            &mut reported,
        );
        if let Some(result) = stepped {
            return (result, reported);
        }
    }
}

/// One step of [`drive`]: steps `run`, appends the step's events to
/// `reported`, hands every effect the step issued to `issued` in issue
/// order, answers each through `perform`, hands each effect and its answer
/// to `answered`, `Dropped` ones included, and returns the run's result
/// once it has one.
///
/// An effect `perform` answers `None` joins `held`, unanswered. Once the
/// run has decided its outcome, every effect it issues and every held one
/// is answered `Dropped`.
pub(crate) fn drive_step(
    run: &mut Run,
    issued: &mut impl FnMut(&Effect),
    perform: &mut impl FnMut(EffectId, &Effect) -> Option<EffectAnswer>,
    answered: &mut impl FnMut(&Effect, &EffectAnswer),
    held: &mut Vec<(EffectId, Effect)>,
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
            if effects.is_empty() && held.is_empty() {
                // Every effect is answered the step it is issued unless it
                // is held, so a pending step that issued nothing while
                // holding nothing has nothing to wait on: an invariant
                // failure reported rather than a hang.
                let error = Error::internal(
                    "the serial driver was handed a pending run with no effect to answer",
                );
                return Some(RunResult::Failure(RunError::from(error)));
            }
            let decided = run.decided();
            for (id, _, effect) in effects {
                issued(&effect);
                let answer = if decided {
                    Some(EffectAnswer::Dropped)
                } else {
                    perform(id, &effect)
                };
                match answer {
                    Some(answer) => {
                        answered(&effect, &answer);
                        run.resume(id, answer);
                    }
                    None => held.push((id, effect)),
                }
            }
            if decided {
                for (id, effect) in held.drain(..) {
                    answered(&effect, &EffectAnswer::Dropped);
                    run.resume(id, EffectAnswer::Dropped);
                }
            }
            None
        }
    }
}
