//! The Rust-backed model tool loop behind the section-visible
//! `models.loop(handle?, messages, compactor?)`.
//!
//! The scheduler drives [`run_models_loop`] on the driver thread: the loop
//! holds the section VM through its append sink and local-tool dispatcher,
//! so it cannot cross a spawned-task boundary. Each round prechecks the
//! projected conversation against the model's context window, runs one
//! streaming gateway completion under cancellation, and either appends the
//! terminal assistant text and returns, or dispatches the requested
//! tool-call batch and appends the exchange - the assistant record and its
//! correlated tool results together, only after every dispatch in the batch
//! succeeded - before looping. Overflow on the precheck or at the provider
//! invokes the selected compactor (the omitted-compactor default is
//! `compactors.fail`, which always raises typed context exhaustion).
//!
//! [`run_prose_inference`] is the test-only wrapper the legacy loop tests
//! keep their call shape through: it pushes one user prose message and
//! captures the terminal record the loop appends.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::AtomicU32;

use promptforge_core_support::events::{CallMetrics, ToolCallEvent};

use crate::cancel;
use crate::client::{
    Completion, CompletionResult, GatewayClient, Message, StreamDelta, ToolSchema,
};
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{
    MessageContent, MessageRecord, MessageRole, OverflowReason, ToolCallCounts, ToolCallRecord,
    dispatch_tool, is_context_overflow, precheck,
};
use crate::model::CompletionOptions;
use crate::observe::{Observer, detail};
use crate::tools::ToolId;
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

/// The terminal assistant record for one completed loop: plain text, no
/// calls, no answered ID.
fn terminal_record(text: String) -> MessageRecord {
    MessageRecord {
        role: MessageRole::Assistant,
        content: MessageContent::Text(text),
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

/// The assistant record for one tool-call round: empty visible text (the
/// client's tool-call outcome carries none) plus the normalized
/// `{id, name, arguments}` calls.
fn assistant_calls_record(calls: &[crate::client::ToolCall]) -> MessageRecord {
    MessageRecord {
        role: MessageRole::Assistant,
        content: MessageContent::Text(String::new()),
        tool_calls: calls
            .iter()
            .map(|call| ToolCallRecord {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect(),
        tool_call_id: None,
    }
}

/// The tool record answering one dispatched call.
fn tool_result_record(id: &str, content: String) -> MessageRecord {
    MessageRecord {
        role: MessageRole::Tool,
        content: MessageContent::Text(content),
        tool_calls: Vec::new(),
        tool_call_id: Some(id.to_owned()),
    }
}

/// Assembles one round's [`CallMetrics`] from everything the completion
/// measured, or `None` when nothing was measured.
fn call_metrics(completion: &Completion) -> Option<CallMetrics> {
    let metrics = CallMetrics {
        usage: completion.usage().cloned(),
        llama: completion.llama_timings().cloned(),
        vllm: completion.vllm_metrics().cloned(),
        client: completion.client_timing().cloned(),
    };
    let measured = metrics.usage.is_some()
        || metrics.llama.is_some()
        || metrics.vllm.is_some()
        || metrics.client.is_some();
    measured.then_some(metrics)
}

/// Loops model inference over `conversation` until the model produces
/// terminal text, appending every assistant message and correlated tool
/// result to the author's message list through `append`.
///
/// The conversation arrives projected from the author's validated records;
/// the loop appends its own wire messages as rounds complete. Each append
/// to the author's list lands as its round completes: a tool-call exchange
/// appends atomically once every dispatch in the batch succeeded, and the
/// terminal assistant text is the final record. Returns `()` on success -
/// the Lua shim resumes nil.
///
/// # Errors
/// Returns an out-of-scope tool error if the model calls an alias absent from
/// `dispatch`, [`Error::ToolLoopExhausted`] if the cap is hit
/// without a text reply, [`Error::Interrupted`]
/// when the run is cancelled, any transport/backend error from a model call
/// or a tool's own failure, or the append sink's own error. Returns the
/// selected compactor's error - typed [`Error::ContextExhausted`] from the
/// `compactors.fail` default - when the pre-dispatch precheck or the
/// provider reports a context-window overflow. Returns [`Error::Internal`]
/// if a local tool call reaches dispatch without the required local
/// dispatcher.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the reporting pieces arrive dissolved from the driver's frame - observer, debug, turns, and completion options are the frame's effective handles; counts and global_aliases extend the loop's borrowed context for per-VM call tracking"
)]
pub(crate) async fn run_models_loop(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, DispatchTarget>,
    conversation: &mut Vec<Message>,
    append: &mut (dyn FnMut(&MessageRecord) -> Result<()> + Send + Sync),
    max_tool_iterations: usize,
    context: NonZeroU32,
    compactor: &(dyn Fn(OverflowReason) -> Error + Send + Sync),
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
    on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
) -> Result<()> {
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas)
    };

    // Completed dispatches only: a tool handler failure aborts the loop, so
    // reaching the next round already proves the earlier calls succeeded.
    let mut successful_tool_calls: usize = 0;

    for _ in 0..max_tool_iterations {
        // The pre-dispatch precheck: estimate the request against the
        // model's context window before anything leaves. Overflow invokes
        // the selected compactor - the omitted-compactor default,
        // `compactors.fail`, always raises typed context exhaustion - and
        // the refused dispatch is observed as a failed turn, matching the
        // projection-failure precedent.
        if let Err(reason) = precheck(conversation, context) {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
            return Err(compactor(reason));
        }
        // The host's delta callback is the live consumer; without one the
        // chunks drop at the leaf and the completed reply is the repair.
        let completion = tokio::select! {
            biased;
            () = cancel::wait_cancelled() => Err(Error::Interrupted),
            result = client.complete(conversation, tool_arg, completion_options, |delta| {
                if let Some(hook) = on_delta {
                    hook(delta);
                }
            }) => result.map_err(Error::from),
        };
        if let Err(Error::Interrupted) = &completion {
            return Err(Error::Interrupted);
        }
        // Provider overflow: the backend rejected the request as too large
        // for the model's context window. The selected compactor answers
        // with typed context exhaustion rather than propagating the bare
        // backend failure.
        if let Err(Error::Backend { status, body }) = &completion
            && is_context_overflow(*status, body)
        {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
            return Err(compactor(OverflowReason::Provider));
        }
        // A turn whose reply is empty is the model's clean exit from the loop
        // when it stopped deliberately (`finish_reason == "stop"`) after doing
        // its work through tool calls; the terminal record is then an empty
        // assistant text. Every other empty turn (no prior tool calls, or a
        // missing/non-"stop" finish reason) stays an `EmptyModelReply`
        // failure.
        if let Err(Error::EmptyModelReply { finish_reason, .. }) = &completion
            && finish_reason.as_deref() == Some("stop")
            && successful_tool_calls > 0
        {
            // The accepted exit is still a completed turn: count it and report
            // it so observers and turn totals match a text-reply exit. No
            // debug capture fires here because the failed completion carries
            // no request/response bodies to record.
            advance_turn(turns);
            observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);
            append(&terminal_record(String::new()))?;
            return Ok(());
        }
        if completion.is_err() {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
        }
        let completion = completion?;

        // A round trip that produced a reply is a turn, whether the reply is
        // the section's final text or a batch of tool calls.
        let turn = advance_turn(turns);
        // Extracted before the debug capture, which moves the request body
        // out of the completion.
        let metrics = call_metrics(&completion);
        let model_name = completion.model().to_owned();
        let thinking = completion
            .reasoning_content()
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        let finish_reason = completion.finish_reason().map(str::to_owned);
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

        // The content reports every host transcript is built from: the
        // thinking side channel first, then the reply or the tool-call
        // batch, each with model and metrics - the agent driver's round
        // reporting, on the unified loop.
        if let Some(thinking) = &thinking {
            observer.on_thinking(execution, section, 0, 0, turn, &model_name, thinking);
        }

        match completion.result {
            CompletionResult::Text(text) => {
                if finish_reason.as_deref() == Some("length") {
                    observer.observe(execution, section, detail::MODEL_TURN_TRUNCATED);
                }
                observer.on_assistant_reply(
                    execution,
                    section,
                    0,
                    0,
                    turn,
                    &text,
                    finish_reason.as_deref(),
                    &model_name,
                    metrics.as_ref(),
                );
                // The terminal assistant text is the final record.
                append(&terminal_record(text))?;
                return Ok(());
            }
            CompletionResult::ToolCalls(calls) => {
                let events: Vec<ToolCallEvent> = calls
                    .iter()
                    .map(|call| ToolCallEvent {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })
                    .collect();
                observer.on_assistant_tool_calls(
                    execution,
                    section,
                    0,
                    0,
                    turn,
                    &model_name,
                    &events,
                );
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
                    let result = match target {
                        DispatchTarget::Local => {
                            if let Some(counts) = counts {
                                counts.increment(&call.name)?;
                            }
                            // Local tools are Lua functions on the section VM;
                            // they carry no attached implementation.
                            let Some(local) = local_dispatch else {
                                observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                                return Err(Error::Internal(
                                    "a local tool call reached the loop with no local dispatcher",
                                ));
                            };
                            // The handler is synchronous Lua on this thread, so
                            // there is no future to race against cancellation;
                            // the VM's instruction hook polls the cancel flag,
                            // so a stuck handler still aborts on cancellation.
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
                            // is trusted and appends verbatim.
                            let text = call_result?;
                            observer.on_tool_result(
                                execution, section, 0, 0, turn, &call.id, &call.name, &text, true,
                            );
                            text
                        }
                        DispatchTarget::Bound(binding) => {
                            // The implementation was attached at bind time, so
                            // dispatch never consults the catalog. The shared
                            // dispatch body owns the cancel race, the counts
                            // increment, the untrusted wrap, and the observer
                            // events, so this loop and the scheduler's
                            // `tools.call` arm cannot drift. Model-initiated
                            // calls pass no script report: the loop reports
                            // the result under the model-issued call id.
                            let outcome = dispatch_tool(
                                binding,
                                call.arguments.clone(),
                                counts,
                                nonce,
                                observer,
                                execution,
                                section,
                                None,
                            )
                            .await
                            .map_err(Error::from)?;
                            observer.on_tool_result(
                                execution,
                                section,
                                0,
                                0,
                                turn,
                                &call.id,
                                &call.name,
                                outcome.content(),
                                outcome.trusted(),
                            );
                            outcome.into_content()
                        }
                    };
                    successful_tool_calls += 1;
                    results.push((call.id.clone(), result));
                }

                // The exchange appends atomically: reaching here means every
                // dispatch in the batch succeeded, so the author's list never
                // holds an assistant call its results did not answer.
                //
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
                append(&assistant_calls_record(&calls))?;
                for (id, content) in results {
                    conversation.push(Message::tool(id.clone(), content.clone()));
                    append(&tool_result_record(&id, content))?;
                }
            }
            // `CompletionResult` is `#[non_exhaustive]` across the crate
            // boundary: an outcome this build does not recognize can be neither
            // dispatched nor promoted to an answer.
            _ => return Err(Error::Internal("unrecognized completion outcome")),
        }
    }

    Err(Error::ToolLoopExhausted)
}

/// Text and finish reason from one tool-loop inference.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct ProseInferenceResult {
    /// Model text when the loop produced a reply.
    pub text: Option<String>,
    /// Backend `finish_reason` from the last completed model round, when present.
    pub finish_reason: Option<String>,
}

/// The test-only wrapper the legacy loop tests keep their call shape
/// through: push `prose` as one user message, run [`run_models_loop`] with
/// a sink that captures the terminal record, and render the captured text.
/// The compactor arrives as the optional typed policy; the omitted default
/// is `compactors.fail`.
///
/// # Errors
/// Exactly [`run_models_loop`]'s, plus [`Error::Internal`] if the loop
/// completed without appending a terminal record.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "the wrapper keeps the deleted production function's borrowed loop context so the loop tests keep their call shape"
)]
pub(crate) async fn run_prose_inference(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, DispatchTarget>,
    conversation: &mut Vec<Message>,
    prose: String,
    max_tool_iterations: usize,
    context: NonZeroU32,
    compactor: Option<crate::lua::Compactor>,
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
    // The loop's last append is the terminal assistant record; capture it
    // through the sink rather than a second return channel.
    let mut terminal: Option<MessageRecord> = None;
    let mut append = |record: &MessageRecord| -> Result<()> {
        terminal = Some(record.clone());
        Ok(())
    };
    let invoke = move |reason: OverflowReason| -> Error {
        compactor.unwrap_or_default().invoke(reason).into()
    };
    run_models_loop(
        client,
        schemas,
        dispatch,
        conversation,
        &mut append,
        max_tool_iterations,
        context,
        &invoke,
        execution,
        observer,
        section,
        turns,
        debug,
        completion_options,
        nonce,
        counts,
        global_aliases,
        local_dispatch,
        None,
    )
    .await?;
    let Some(record) = terminal else {
        return Err(Error::Internal(
            "a completed loop appended no terminal record",
        ));
    };
    let MessageContent::Text(text) = record.content else {
        return Err(Error::Internal("the terminal record is always plain text"));
    };
    Ok(ProseInferenceResult {
        text: Some(text),
        finish_reason: None,
    })
}
