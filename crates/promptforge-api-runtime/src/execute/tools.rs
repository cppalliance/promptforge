//! The nested-inference round a section's `models.infer` yields resolve to.
//!
//! One `infer` shape only: a single direct, tool-free gateway round on a
//! fresh conversation. `models.infer(handle, prompt)` runs it with the
//! handle's frozen binding; `models.infer(prompt)` resolves the section's
//! current model and runs the same path. Neither form advertises tools, sets
//! `reply`, or touches `sys`. A Lua block that needs tools uses `call`
//! on a section. The scheduler's leaf dispatch spawns the round and resumes
//! the yielding chain with its outcome.

use std::sync::atomic::AtomicU32;

use crate::Error;
use crate::client::{Completion, CompletionResult, GatewayClient, Message};
use crate::model::ModelBinding;
use crate::observe::detail;

use super::event_buffer::Emitter;
use super::support::advance_turn;

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
    emitter.report(section, detail::MODEL_TURN_COMPLETED);

    match completion.result {
        CompletionResult::Text(text) => {
            if completion.finish_reason.as_deref() == Some("length") {
                emitter.report(section, detail::MODEL_TURN_TRUNCATED);
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

/// The one infer shape as an async round: a single direct, tool-free
/// gateway call on a fresh conversation with `binding`, reported exactly
/// like one prose round.
///
/// The scheduler's leaf dispatch drives this on a spawned task, so
/// cancellation is the driver aborting the task mid-round - no
/// `MODEL_TURN_FAILED` fires for an aborted round. The round reports
/// through the chain's `emitter` under `section`.
pub(crate) async fn infer_round(
    client: &GatewayClient,
    binding: &ModelBinding,
    prompt: &str,
    emitter: &Emitter,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let completion_options = binding.completion_options();
    let conversation = [Message::user(prompt)];
    // A nested infer round consumes only the accumulated completion; live
    // deltas have no consumer here, so the callback is a no-op.
    let completion = match client
        .complete(&conversation, None, &completion_options, |_| {})
        .await
    {
        Ok(completion) => completion,
        Err(error) => {
            emitter.report(section, detail::MODEL_TURN_FAILED);
            return Err(Error::from(error));
        }
    };
    accept_infer_completion(completion, emitter, section, turns)
}
