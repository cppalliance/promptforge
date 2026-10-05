//! The suites' chat client: a scripted model played in process.
//!
//! The Engine emits each round as a `Chat` effect; the Harness performs it
//! with its own gateway client, and this crate may not name a Harness crate.
//! [`ScriptedChat`] answers each round from a fixed script instead, with
//! no HTTP and no wire code, and records what every round carried so a
//! suite asserts on the effect rather than on a request body. The answers
//! follow the rules the gateway client's reader applies: an empty reply is
//! the reader's `EmptyReply` failure, and a reply past the run's timeout is
//! the client's `Timeout` failure.
//!
//! This file names only external crates so the bench target can include
//! it by `#[path]` beside the in-crate suites; it is not part of the
//! `test-support` feature.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use promptforge_model_client::client::{
    Completion, CompletionResult, Message, ToolCall, ToolSchema,
};
use promptforge_model_client::model::{CompletionError, CompletionErrorKind, CompletionOptions};
use promptforge_types::metrics::CallMetrics;
use serde_json::Value;

/// The specific the reader adds to an empty reply whose reasoning it
/// ignored as answer text.
const REASONING_IGNORED: &str = "reasoning content was present but ignored";

/// One scripted answer to a round.
#[derive(Clone, Debug)]
pub(crate) enum ScriptedReply {
    /// A text reply. Whitespace-only `content` is an empty reply.
    Text {
        /// The model the completion names as its server.
        model: String,
        /// The reply text.
        content: String,
        /// The backend's stop label, when it supplied one.
        finish_reason: Option<String>,
        /// The reasoning side channel, when the backend supplied one.
        reasoning: Option<String>,
        /// What the backend measured, when it reported anything.
        metrics: Option<Box<CallMetrics>>,
    },
    /// A tool-call batch: each call's id, wire name, and parsed arguments.
    ToolCalls {
        /// The model the completion names as its server.
        model: String,
        /// The calls, in batch order.
        calls: Vec<(String, String, Value)>,
    },
    /// A failed round. A fresh [`CompletionError`] is built for every
    /// answer, because the error is not `Clone`.
    Failure {
        /// The failure's kind.
        kind: CompletionErrorKind,
        /// The failure's message.
        message: String,
        /// The provider text behind the failure, when there is some.
        detail: Option<String>,
        /// The backend's stop label, when it supplied one.
        finish_reason: Option<String>,
    },
    /// Another reply, answered after a delay the run's timeout bounds.
    Delayed(Duration, Box<ScriptedReply>),
}

impl ScriptedReply {
    /// The total delay before this reply is answered.
    fn delay(&self) -> Duration {
        match self {
            ScriptedReply::Delayed(delay, inner) => *delay + inner.delay(),
            _ => Duration::ZERO,
        }
    }

    /// Answers a round with this reply.
    fn answer(&self) -> Result<Completion, CompletionError> {
        match self {
            ScriptedReply::Text {
                model,
                content,
                finish_reason,
                reasoning,
                metrics,
            } => {
                let reasoning = reasoning.as_deref().filter(|text| !text.is_empty());
                if content.trim().is_empty() {
                    return Err(empty_reply(reasoning.is_some(), finish_reason.as_deref()));
                }
                let mut completion =
                    Completion::from_result(CompletionResult::Text(content.clone()), model)?;
                if let Some(reason) = finish_reason {
                    completion = completion.with_finish_reason(reason);
                }
                if let Some(reasoning) = reasoning {
                    completion = completion.with_reasoning_content(reasoning);
                }
                if let Some(metrics) = metrics {
                    completion = completion.with_metrics(metrics.as_ref().clone());
                }
                Ok(completion)
            }
            ScriptedReply::ToolCalls { model, calls } => {
                let calls = calls
                    .iter()
                    .map(|(id, name, arguments)| ToolCall::from_parts(id, name, arguments.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                Completion::from_result(CompletionResult::ToolCalls(calls), model)
            }
            ScriptedReply::Failure {
                kind,
                message,
                detail,
                finish_reason,
            } => {
                let mut error = CompletionError::new(*kind, message);
                if let Some(detail) = detail {
                    error = error.with_detail(detail);
                }
                if let Some(reason) = finish_reason {
                    error = error.with_finish_reason(reason);
                }
                Err(error)
            }
            ScriptedReply::Delayed(_, inner) => inner.answer(),
        }
    }
}

/// The empty-reply failure the reader raises for a turn with no text and
/// no tool calls.
fn empty_reply(reasoning_present: bool, finish_reason: Option<&str>) -> CompletionError {
    let phrase = CompletionErrorKind::EmptyReply.phrase();
    let message = if reasoning_present {
        format!("{phrase}: {REASONING_IGNORED}")
    } else {
        phrase.to_owned()
    };
    let error = CompletionError::new(CompletionErrorKind::EmptyReply, message);
    match finish_reason {
        Some(reason) => error.with_finish_reason(reason),
        None => error,
    }
}

/// What one round's `Chat` effect carried.
#[derive(Clone, Debug)]
pub(crate) struct ScriptedCall {
    /// The conversation the round sent.
    pub(crate) messages: Vec<Message>,
    /// The tools the round advertised, in order.
    pub(crate) tools: Vec<ToolSchema>,
    /// The options the round ran under.
    pub(crate) options: CompletionOptions,
}

impl ScriptedCall {
    /// The round's messages as JSON, in the shape each message serializes
    /// to.
    pub(crate) fn messages_json(&self) -> Value {
        serde_json::to_value(&self.messages).expect("a message serializes to JSON")
    }

    /// The wire names of the tools the round advertised, in order.
    pub(crate) fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(ToolSchema::name).collect()
    }
}

/// A scripted model: it answers rounds from its script in order,
/// repeating the last reply once the script is exhausted, counts them,
/// and records what each carried. Clones share the script, the count, and
/// the record.
#[derive(Clone, Debug)]
pub(crate) struct ScriptedChat {
    replies: Arc<Vec<ScriptedReply>>,
    requests: Arc<Mutex<Vec<ScriptedCall>>>,
    /// The number of rounds asked of the script so far.
    pub(crate) calls: Arc<AtomicUsize>,
}

impl ScriptedChat {
    /// A model answering with `replies` in order.
    ///
    /// # Panics
    /// Panics when `replies` is empty.
    pub(crate) fn new(replies: Vec<ScriptedReply>) -> ScriptedChat {
        assert!(
            !replies.is_empty(),
            "a scripted chat needs at least one reply"
        );
        ScriptedChat {
            replies: Arc::new(replies),
            requests: Arc::new(Mutex::new(Vec::new())),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The number of rounds asked of the script so far.
    pub(crate) fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Every recorded round, in the order the rounds arrived.
    pub(crate) fn requests(&self) -> Vec<ScriptedCall> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The most recently recorded round, if any.
    pub(crate) fn last_request(&self) -> Option<ScriptedCall> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .cloned()
    }

    /// Answers one round: records what it carried, waits out a delayed
    /// reply under `timeout`, and returns the completion or the failure.
    /// `max_bytes` bounds nothing, because no bytes are read.
    ///
    /// # Errors
    /// Returns the scripted failure, the `Timeout` failure for a delay
    /// past `timeout`, the `EmptyReply` failure for an empty text reply,
    /// and the `MalformedResponse` failure for a tool call the wire
    /// decoder would refuse.
    pub(crate) async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        options: &CompletionOptions,
        timeout: Duration,
        _max_bytes: NonZeroU64,
    ) -> Result<Completion, CompletionError> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ScriptedCall {
                messages: messages.to_vec(),
                tools: tools.to_vec(),
                options: options.clone(),
            });
        let reply = &self.replies[index.min(self.replies.len() - 1)];
        let delay = reply.delay();
        if !delay.is_zero()
            && tokio::time::timeout(timeout, tokio::time::sleep(delay))
                .await
                .is_err()
        {
            let kind = CompletionErrorKind::Timeout;
            return Err(CompletionError::new(kind, kind.phrase()));
        }
        reply.answer()
    }
}
