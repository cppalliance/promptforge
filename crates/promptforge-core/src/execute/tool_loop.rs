//! The per-section model tool loop and single prose-inference round driver.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU32;

use crate::cancel;
use crate::client::{CompletionResult, GatewayClient, Message, ToolSchema};
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::ToolCallCounts;
use crate::model::CompletionOptions;
use crate::observe::{Observer, detail};
use crate::tools::{ToolId, ToolOutput};
use crate::untrusted::GuardNonce;
use crate::{Error, Result};

use super::scope::DispatchTarget;
use super::support::advance_turn;

/// Routes a local (Lua-registered) tool call back into its section VM.
///
/// Local tools are prompt-author Lua functions with no live implementation;
/// the loop dispatches them through this closure instead of a bound tool. The
/// closure takes the tool alias and the call's JSON arguments and returns
/// the handler's rendered string result.
pub(crate) type LocalDispatch<'a> =
    dyn Fn(&str, serde_json::Value) -> Result<String> + Send + Sync + 'a;

/// How many model rounds a prose block may take.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ProseMode {
    /// One model round; tool calls for that round are dispatched, then control
    /// returns even without a final text reply.
    SingleShot,
    /// Keep calling until text or `max_tool_iterations` is exhausted.
    Loop { max_tool_iterations: usize },
}

/// Text and finish reason from one prose or tool-loop inference.
#[derive(Debug, Clone)]
pub(crate) struct ProseInferenceResult {
    /// Model text when the round produced a reply; `None` for single-shot tool rounds.
    pub text: Option<String>,
    /// Backend `finish_reason` from the last completed model round, when present.
    pub finish_reason: Option<String>,
}

/// Append `prose` to `conversation` and run model inference under `mode`.
///
/// Returns text when the model produces it. For [`ProseMode::SingleShot`],
/// text may be `None` after one round that only issued tool calls. In the
/// loop mode, an empty reply with `finish_reason == "stop"` after at least
/// one successful tool dispatch is accepted as a clean exit with empty text.
/// Conversation history accumulates for later prose blocks.
///
/// # Errors
/// Returns an out-of-scope tool error if the model calls an alias absent from
/// `dispatch`, [`Error::ToolLoopExhausted`] in loop mode if the cap is hit
/// without a text reply (single-shot never reports it), [`Error::Interrupted`]
/// when the run is cancelled, or any transport/backend error from a model call
/// or a tool's own failure. Returns [`Error::Internal`] if a local tool call
/// reaches dispatch without the required local dispatcher.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the reporting pieces arrive dissolved from the driver's frame - observer, debug, turns, and completion options are the frame's effective handles; counts and global_aliases extend the loop's borrowed context for per-VM call tracking"
)]
pub(crate) async fn run_prose_inference(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, DispatchTarget>,
    conversation: &mut Vec<Message>,
    prose: String,
    mode: ProseMode,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
    turns: &AtomicU32,
    debug: Option<&dyn DebugCapture>,
    completion_options: &CompletionOptions,
    nonce: &GuardNonce,
    counts: Option<&ToolCallCounts>,
    global_aliases: Option<&BTreeMap<String, ToolId>>,
    local_dispatch: Option<&LocalDispatch<'_>>,
) -> Result<ProseInferenceResult> {
    conversation.push(Message::user(prose));
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas)
    };

    let max_tool_iterations = match mode {
        ProseMode::SingleShot => 1,
        ProseMode::Loop {
            max_tool_iterations,
        } => max_tool_iterations,
    };

    // Completed dispatches only: a tool handler failure aborts the loop, so
    // reaching the next round already proves the earlier calls succeeded.
    let mut successful_tool_calls: usize = 0;

    for _ in 0..max_tool_iterations {
        let completion = tokio::select! {
            biased;
            () = cancel::wait_cancelled() => Err(Error::Interrupted),
            result = client.complete(conversation, tool_arg, completion_options) => result.map_err(Error::from),
        };
        if let Err(Error::Interrupted) = &completion {
            return Err(Error::Interrupted);
        }
        // A turn whose reply is empty is the model's clean exit from the loop
        // when it stopped deliberately (`finish_reason == "stop"`) after doing
        // its work through tool calls; the section reply is then "". Every
        // other empty turn (no prior tool calls, or a missing/non-"stop"
        // finish reason) stays an `EmptyModelReply` failure. SingleShot needs
        // no clause: its sole round is the first turn, where the dispatch
        // count is always zero, so the conditions can never hold there.
        if let Err(Error::EmptyModelReply { finish_reason, .. }) = &completion
            && finish_reason.as_deref() == Some("stop")
            && successful_tool_calls > 0
        {
            let finish_reason = finish_reason.clone();
            // The accepted exit is still a completed turn: count it and report
            // it so observers and turn totals match a text-reply exit. No
            // debug capture fires here because the failed completion carries
            // no request/response bodies to record.
            advance_turn(turns);
            observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);
            return Ok(ProseInferenceResult {
                text: Some(String::new()),
                finish_reason,
            });
        }
        if completion.is_err() {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
        }
        let completion = completion?;

        // A round trip that produced a reply is a turn, whether the reply is
        // the section's final text or a batch of tool calls.
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
                return Ok(ProseInferenceResult {
                    text: Some(text),
                    finish_reason: completion.finish_reason,
                });
            }
            CompletionResult::ToolCalls(calls) => {
                let finish_reason = completion.finish_reason.clone();
                // Dispatch each requested tool and collect the framed results
                // as (call id, content) pairs, in call order.
                let mut results: Vec<(String, String)> = Vec::with_capacity(calls.len());
                for call in &calls {
                    let Some(target) = dispatch.get(&call.name) else {
                        observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                        let global_exists =
                            global_aliases.is_some_and(|g| g.contains_key(&call.name));
                        let in_scope: Vec<String> = dispatch.keys().cloned().collect();
                        return Err(Error::OutOfScopeToolCall {
                            name: call.name.clone(),
                            global_exists,
                            in_scope,
                        });
                    };
                    if let Some(counts) = counts {
                        counts.increment(&call.name)?;
                    }
                    let output = match target {
                        DispatchTarget::Local => {
                            // Local tools are Lua functions on the section VM;
                            // they carry no attached implementation.
                            let Some(local) = local_dispatch else {
                                observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                                return Err(Error::Internal(
                                    "a local tool call reached the loop with no local dispatcher",
                                ));
                            };
                            // The handler is synchronous Lua on this thread, so
                            // there is no future to race against cancellation; a
                            // stuck handler is bounded by the VM's instruction
                            // budget instead.
                            let call_result = local(&call.name, call.arguments.clone());
                            observer.observe(
                                execution,
                                section,
                                if call_result.is_ok() {
                                    detail::TOOL_CALL_SUCCEEDED
                                } else {
                                    detail::TOOL_CALL_FAILED
                                },
                            );
                            // The prompt author wrote the handler, so its output
                            // is trusted.
                            ToolOutput::trusted(call_result?)
                        }
                        DispatchTarget::Bound(binding) => {
                            // The implementation was attached at bind time, so
                            // dispatch never consults the catalog.
                            let tool = binding.tool();
                            // Race the tool call against cancellation so a slow or stuck
                            // tool cannot hold the run past a Ctrl-C. On cancel the tool
                            // future is dropped and the run ends promptly.
                            let call_result = tokio::select! {
                                biased;
                                () = cancel::wait_cancelled() => {
                                    observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                                    return Err(Error::Interrupted);
                                }
                                result = tool.call(call.arguments.clone()) => result,
                            };
                            observer.observe(
                                execution,
                                section,
                                if call_result.is_ok() {
                                    detail::TOOL_CALL_SUCCEEDED
                                } else {
                                    detail::TOOL_CALL_FAILED
                                },
                            );
                            call_result.map_err(Error::tool)?
                        }
                    };
                    successful_tool_calls += 1;
                    // Trust travels with the output: an untrusted result is
                    // nonce-wrapped before it can reach the next model turn. Every
                    // wrap in the run shares the run's nonce, so identical content
                    // yields a byte-identical envelope and KV-cache prefixes stay
                    // shared across rounds and fanout arms; the `<`-escaping is
                    // what actually blocks a forged close tag, so the reuse costs
                    // nothing.
                    let result = match output.trust() {
                        crate::tools::OutputTrust::Trusted => output.text().to_owned(),
                        // `OutputTrust` is `#[non_exhaustive]` in the contract
                        // crate: an unknown future variant takes the safe path
                        // and is nonce-wrapped as untrusted.
                        _ => nonce.wrap(output.text()),
                    };
                    results.push((call.id.clone(), result));
                }

                // Echo in the OpenAI wire shape: the assistant's tool-call turn
                // followed by one `role=tool` message per result. The assistant
                // turn is a canonical, deliberately lossy reconstruction of each
                // call - exactly `{ "id", "type": "function", "function": {
                // "name", "arguments" } }` with `arguments` as the compact JSON
                // string of the parsed object - because `ToolCall` retains only
                // the validated `id`, `name`, and `arguments`, and this canonical
                // subset is what backends require to continue a tool loop.
                let raw_calls: Vec<serde_json::Value> = calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            },
                        })
                    })
                    .collect();
                conversation.push(Message::assistant_tool_calls(raw_calls));
                for (id, content) in results {
                    conversation.push(Message::tool(id, content));
                }
                if matches!(mode, ProseMode::SingleShot) {
                    return Ok(ProseInferenceResult {
                        text: None,
                        finish_reason,
                    });
                }
            }
            // `CompletionResult` is `#[non_exhaustive]` across the crate
            // boundary: an outcome this build does not recognize can be neither
            // dispatched nor promoted to an answer.
            _ => return Err(Error::Internal("unrecognized completion outcome")),
        }
    }

    match mode {
        ProseMode::SingleShot => Ok(ProseInferenceResult {
            text: None,
            finish_reason: None,
        }),
        ProseMode::Loop { .. } => Err(Error::ToolLoopExhausted),
    }
}
