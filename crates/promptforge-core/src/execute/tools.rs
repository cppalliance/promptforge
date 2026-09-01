//! The nested-inference round a section's `models.infer`/`handle:infer`
//! yields resolve to.
//!
//! One `infer` shape only: a single direct, tool-free gateway round on a
//! fresh conversation. `handle:infer(prompt)` runs it with the handle's
//! frozen binding; `models.infer(prompt)` resolves the section's current
//! model and runs the same path. Neither form advertises tools, sets
//! `reply`, or touches `sys`. A Lua block that needs tools uses `execute`
//! on a section. The scheduler's leaf dispatch spawns the round and resumes
//! the yielding chain with its outcome.

use std::sync::atomic::AtomicU32;

use crate::Error;
use crate::client::{Completion, CompletionResult, GatewayClient, Message};
use crate::debug::{DebugCapture, DebugEvent};
use crate::model::ModelBinding;
use crate::observe::{Observer, detail};

use super::support::advance_turn;

/// Reports one completed infer round exactly like a single prose round and
/// renders its text: the turn advance, the debug capture pair, the
/// completion and truncation observations, and the no-tools-advertised
/// violation check.
fn accept_infer_completion(
    completion: Completion,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    execution: &str,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let turn = advance_turn(turns);
    if let Some(capture) = debug {
        capture.on_event(
            execution,
            section,
            turn,
            DebugEvent::Request {
                body: completion.request_body,
            },
        );
        capture.on_event(
            execution,
            section,
            turn,
            DebugEvent::Response {
                body: completion.response_body.clone(),
                finish_reason: completion.finish_reason.clone(),
                reasoning_content: completion.reasoning_content.clone(),
            },
        );
    }
    observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);

    match completion.result {
        CompletionResult::Text(text) => {
            if completion.finish_reason.as_deref() == Some("length") {
                observer.observe(execution, section, detail::MODEL_TURN_TRUNCATED);
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
/// `MODEL_TURN_FAILED` fires for an aborted round.
#[expect(
    clippy::too_many_arguments,
    reason = "the one infer round keeps the client, binding, prompt, and the frame's reporting handles explicit and linear"
)]
pub(crate) async fn infer_round(
    client: &GatewayClient,
    binding: &ModelBinding,
    prompt: &str,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    execution: &str,
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
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
            return Err(Error::from(error));
        }
    };
    accept_infer_completion(completion, observer, debug, execution, section, turns)
}
