//! The nested-inference round a section's `models.infer` yields resolve to.
//!
//! One `infer` shape only: a single direct, tool-free gateway round on a
//! fresh conversation. `h:infer(prompt)` runs it with the handle's
//! frozen binding; `models.infer(prompt)` resolves the section's
//! current model and runs the same path. Neither form advertises tools, sets
//! `reply`, or touches `sys`. A Lua block that needs tools uses `call`
//! on a section. The scheduler's leaf dispatch issues the round as a
//! `Chat` effect over one user message and no tools; when the answer
//! arrives, [`accept_infer`] reports the round and renders its text, and
//! the yielding chain resumes with the outcome.

use std::sync::atomic::AtomicU32;

use promptforge_types::emitter::Emitter;
use promptforge_types::event::ReplyOrigin;
use promptforge_types::event::lifecycle;
use promptforge_types::ids::RoundId;

use crate::Error;
use crate::model::{Completion, CompletionError, CompletionErrorKind, CompletionResult};

use super::turn;

/// Reports one completed infer round through the shared round report and
/// renders its text. The report fires the turn advance's round events - the
/// debug capture pair, the completed boundary, the thinking side channel,
/// the `length` truncation observation - plus the [`Emitter::assistant_reply`]
/// content report tagged `origin = infer`, each content report stamped
/// with `round`. The no-tools-advertised violation check is what keeps the
/// round tool-free.
fn accept_infer_completion(
    completion: Completion,
    round: RoundId,
    emitter: &Emitter,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let turn = turn::advance_turn(turns);
    let (outcome, _) = turn::report_model_turn(
        emitter,
        section,
        turn,
        round,
        completion,
        ReplyOrigin::Infer,
    );
    match outcome {
        CompletionResult::Text(text) => Ok(text),
        // No tools were advertised, so a tool-call turn is a backend
        // protocol violation rather than something to dispatch.
        // `CompletionResult` is `#[non_exhaustive]` across the crate boundary:
        // an unrecognized future outcome is the same violation.
        CompletionResult::ToolCalls(_) => Err(malformed_reply(
            "the round sent tool calls, but no tools were advertised",
        )),
        _ => Err(malformed_reply(
            "the round sent an unrecognized outcome, and no tools were advertised",
        )),
    }
}

/// The malformed-reply failure for an infer round whose outcome breaks the
/// round's protocol, with `specific` naming how.
fn malformed_reply(specific: &str) -> Error {
    let kind = CompletionErrorKind::MalformedResponse;
    Error::Completion(CompletionError::new(
        kind,
        format!("{}: {specific}", kind.phrase()),
    ))
}

/// Applies one infer round's answer - the completion the performer
/// obtained, or its failure - reporting it exactly like one prose round
/// of `round` through the chain's `emitter` under `section`, and renders
/// its text.
///
/// A failed completion is a failed turn and the call's error. The
/// performer's task is aborted on cancellation before any answer lands,
/// so no `MODEL_TURN_FAILED` fires for an aborted round.
///
/// # Errors
/// Returns the completion's failure, or an [`Error::Completion`] of kind
/// `MalformedResponse` when the round produced tool calls (none were
/// advertised) or an unrecognized outcome.
pub(super) fn accept_infer(
    result: std::result::Result<Box<Completion>, CompletionError>,
    round: RoundId,
    emitter: &Emitter,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let completion = match result {
        Ok(completion) => completion,
        Err(error) => {
            emitter.report(section, lifecycle::MODEL_TURN_FAILED);
            return Err(Error::from(error));
        }
    };
    accept_infer_completion(*completion, round, emitter, section, turns)
}

#[cfg(test)]
#[path = "infer-tests.rs"]
mod tests;
