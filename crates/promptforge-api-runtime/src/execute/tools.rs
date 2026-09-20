//! The nested-inference round a section's `models.infer` yields resolve to.
//!
//! One `infer` shape only: a single direct, tool-free gateway round on a
//! fresh conversation. `models.infer(handle, prompt)` runs it with the
//! handle's frozen binding; `models.infer(prompt)` resolves the section's
//! current model and runs the same path. Neither form advertises tools, sets
//! `reply`, or touches `sys`. A Lua block that needs tools uses `call`
//! on a section. The scheduler's leaf dispatch issues the round as a
//! `Chat` effect over one user message and no tools; when the answer
//! arrives, [`accept_infer`] reports the round and renders its text, and
//! the yielding chain resumes with the outcome.

use std::sync::atomic::AtomicU32;

use crate::Error;
use crate::model::{Completion, CompletionError, CompletionResult};
use promptforge_api_types::event::lifecycle;

use super::support::advance_turn;
use promptforge_api_types::emitter::Emitter;

/// Reports one completed infer round exactly like a single prose round and
/// renders its text: the turn advance, the debug capture pair, the
/// completion and truncation events, and the no-tools-advertised
/// violation check.
fn accept_infer_completion(
    completion: Completion,
    emitter: &Emitter,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let turn = advance_turn(turns);
    if emitter.captures_debug() {
        emitter.request(section, turn, completion.request_body);
        emitter.response(
            section,
            turn,
            completion.response_body.clone(),
            completion.finish_reason.clone(),
            completion.reasoning_content.clone(),
        );
    }
    emitter.report(section, lifecycle::MODEL_TURN_COMPLETED);

    match completion.result {
        CompletionResult::Text(text) => {
            if completion.finish_reason.as_deref() == Some("length") {
                emitter.report(section, lifecycle::MODEL_TURN_TRUNCATED);
            }
            Ok(text)
        }
        // No tools were advertised, so a tool-call turn is a backend
        // protocol violation rather than something to dispatch.
        // `CompletionResult` is `#[non_exhaustive]` across the crate boundary:
        // an unrecognized future outcome is the same violation.
        CompletionResult::ToolCalls(_) => Err(Error::Lua(
            "model inference received tool calls but no tools were advertised".to_owned(),
        )),
        _ => Err(Error::Lua(
            "model inference received an unrecognized outcome but no tools were advertised"
                .to_owned(),
        )),
    }
}

/// Applies one infer round's answer - the completion the performer
/// obtained, or its failure - reporting it exactly like one prose round
/// through the chain's `emitter` under `section`, and renders its text.
///
/// A failed completion is a failed turn and the call's error. The
/// performer's task is aborted on cancellation before any answer lands,
/// so no `MODEL_TURN_FAILED` fires for an aborted round.
///
/// # Errors
/// Returns the completion's failure, or [`Error::Lua`] when the round
/// produced tool calls (none were advertised) or an unrecognized outcome.
pub(crate) fn accept_infer(
    result: std::result::Result<Box<Completion>, CompletionError>,
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
    accept_infer_completion(*completion, emitter, section, turns)
}
