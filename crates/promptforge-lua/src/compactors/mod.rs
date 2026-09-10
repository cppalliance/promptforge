//! The minimum compactor surface: overflow reasons, the one shipped policy,
//! the pre-dispatch precheck, and provider-overflow detection.
//!
//! A model request can fail to fit the model's context window twice: the
//! pre-dispatch [`precheck`] can estimate the projected conversation past
//! the window before anything leaves, and the provider can reject the
//! request as too large ([`is_context_overflow`]). Either path invokes the
//! selected [`Compactor`] with the [`OverflowReason`]. `compactors.fail` is
//! the only shipped policy and the omitted-compactor default; it always
//! raises typed context exhaustion ([`Error::ContextExhausted`]).
//! Replacement-returning custom callbacks, budget records, replacement
//! validation, measurable progress, bounded retry, and in-place history
//! replacement belong to the deferred compactor framework.
//!
//! The surface lives in this crate for the same reason the projection does:
//! it owns the message records, both dispatch points (the agent today, the
//! executor's `models.loop` next) depend on it, and the agent cannot depend
//! on the executor.

use mlua::{Function, RegistryKey, Table};
use serde_json::Value;

use super::{Error, Lua, NonZeroU32, Result};
use promptforge_model_client::client::Message;

/// Why a request cannot fit the model's context window.
///
/// The reason is the whole active compactor contract: the invocation passes
/// it to the selected policy, and `compactors.fail` raises it as typed
/// context exhaustion. Budget records and richer detail belong to the
/// deferred compactor framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowReason {
    /// The pre-dispatch estimate exceeds the model's context window; no
    /// request left the host.
    Precheck,
    /// The provider rejected the request as too large for the model's
    /// context window.
    Provider,
}

impl OverflowReason {
    /// Parses the invocation tag the compactor callback receives.
    fn from_tag(tag: &str) -> Option<OverflowReason> {
        match tag {
            "precheck" => Some(OverflowReason::Precheck),
            "provider" => Some(OverflowReason::Provider),
            _ => None,
        }
    }

    /// The invocation tag the compactor callback receives.
    fn tag(self) -> &'static str {
        match self {
            OverflowReason::Precheck => "precheck",
            OverflowReason::Provider => "provider",
        }
    }
}

impl std::fmt::Display for OverflowReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            OverflowReason::Precheck => {
                "the request precheck overflowed the model's context window"
            }
            OverflowReason::Provider => {
                "the provider rejected the request as exceeding the model's context window"
            }
        };
        formatter.write_str(detail)
    }
}

/// The compactor policies the active surface ships.
///
/// `Fail` is the only policy and the omitted-compactor default: invoked
/// with the overflow reason, it always raises typed context exhaustion.
/// Custom replacement callbacks belong to the deferred compactor framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compactor {
    /// `compactors.fail`: always raises [`Error::ContextExhausted`].
    #[default]
    Fail,
}

impl Compactor {
    /// Invokes the policy on one overflow. The only shipped policy always
    /// fails, so the invocation is the typed exhaustion error itself; the
    /// deferred framework generalizes this into a replacement-returning
    /// callback.
    #[must_use]
    pub fn invoke(self, reason: OverflowReason) -> Error {
        match self {
            Compactor::Fail => Error::ContextExhausted { reason },
        }
    }
}

/// Rough characters-per-token divisor for the pre-dispatch estimate.
///
/// The estimate only ever gates the compactor invocation, never a request's
/// content, so a conservative heuristic suffices: four characters per token
/// tracks prose and code closely enough that a conversation well under the
/// window passes and one well over it compacts.
const CHARS_PER_TOKEN: u64 = 4;

/// Per-message framing overhead, in tokens: the role, separators, and
/// tool-call envelope a serialized message adds beyond its text.
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;

/// The pre-dispatch context precheck: estimate the request's prompt tokens
/// from the projected wire messages and refuse the dispatch when the
/// estimate exceeds the model's context window.
///
/// The estimate counts message text and tool-call argument characters; an
/// image part contributes nothing (its token cost bears no relation to the
/// data-URI length), so an image-heavy conversation can still overflow at
/// the provider - the provider-overflow path covers it.
///
/// # Errors
/// Returns [`OverflowReason::Precheck`] when the estimate exceeds the
/// window.
pub fn precheck(
    messages: &[Message],
    context: NonZeroU32,
) -> std::result::Result<(), OverflowReason> {
    if estimate_tokens(messages) > u64::from(context.get()) {
        return Err(OverflowReason::Precheck);
    }
    Ok(())
}

/// The request's estimated prompt tokens: text characters over
/// [`CHARS_PER_TOKEN`], plus per-message framing overhead.
fn estimate_tokens(messages: &[Message]) -> u64 {
    let chars: u64 = messages.iter().map(message_chars).sum();
    chars / CHARS_PER_TOKEN + MESSAGE_OVERHEAD_TOKENS * messages.len() as u64
}

/// One message's estimated character weight: its text (a plain string, or
/// the text parts of a multimodal array) plus its serialized tool calls.
fn message_chars(message: &Message) -> u64 {
    let content = match message.content_value() {
        Value::String(text) => text.len() as u64,
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(|text| text.len() as u64)
            .sum(),
        _ => 0,
    };
    let calls: u64 = message.raw_tool_calls().map_or(0, |calls| {
        calls.iter().map(|call| call.to_string().len() as u64).sum()
    });
    content + calls
}

/// Body signatures the known backends emit when a request exceeds the
/// model's context window (OpenAI and compatible gateways, llama.cpp,
/// vLLM), matched case-insensitively against the bounded, escaped body the
/// client retained.
const OVERFLOW_SIGNATURES: &[&str] = &[
    "context length",
    "context window",
    "context size",
    "context_length_exceeded",
    "too many tokens",
    "prompt is too long",
];

/// Provider-overflow detection: true when a non-success status is a client
/// rejection (400 or 413) whose body names a context-window limit. A 5xx
/// never classifies: an overflow the backend reports as its own error is
/// indistinguishable from a fault, so it stays a plain backend failure.
#[must_use]
pub fn is_context_overflow(status: u16, body: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }
    let body = body.to_lowercase();
    OVERFLOW_SIGNATURES
        .iter()
        .any(|signature| body.contains(signature))
}

/// Invokes the selected compactor on one overflow and returns the error the
/// loop raises.
///
/// The omitted compactor (`None`) defaults to `compactors.fail`, invoked
/// directly: the only shipped policy always raises typed context exhaustion,
/// so the default needs no Lua round trip. An author-selected callback
/// (`Some`, stashed by the loop request's parse) is invoked with the reason
/// tag; `compactors.fail` raises [`Error::ContextExhausted`] across the Lua
/// boundary as a downcastable external error (LUA-012), recovered here as
/// the typed value. A callback that returns instead of raising is the
/// deferred replacement-compactor shape, which the active surface rejects.
///
/// `#[doc(hidden)]`: a cross-crate seam for the executor's `models.loop`
/// driver, not host API.
#[doc(hidden)]
#[must_use]
pub fn invoke_selected(
    lua: &Lua,
    compactor: Option<&RegistryKey>,
    reason: OverflowReason,
) -> Error {
    let Some(key) = compactor else {
        return Compactor::Fail.invoke(reason);
    };
    let function: Function = match lua.registry_value(key) {
        Ok(function) => function,
        Err(error) => return Error::lua(error),
    };
    match function.call::<()>(reason.tag()) {
        Ok(()) => Error::Lua(
            "the selected compactor returned without raising: replacement compactors are \
             deferred; compactors.fail is the only shipped policy"
                .to_owned(),
        ),
        Err(error) => {
            // mlua wraps a callback's error in CallbackError for the
            // traceback; the compactor's typed raise rides as its cause.
            let cause = match &error {
                mlua::Error::CallbackError { cause, .. } => cause.as_ref(),
                other => other,
            };
            match cause {
                mlua::Error::ExternalError(cause) => match cause.downcast_ref::<Error>() {
                    Some(Error::ContextExhausted { reason }) => {
                        Error::ContextExhausted { reason: *reason }
                    }
                    _ => Error::lua(error),
                },
                _ => Error::lua(error),
            }
        }
    }
}

/// Installs the `compactors` global carrying the shipped policies.
///
/// `compactors.fail` is a Rust-backed function: invoked with the overflow
/// reason tag, it raises typed context exhaustion as an external error, so
/// the [`Error::ContextExhausted`] value crosses the Lua boundary
/// downcastable rather than flattened to text (LUA-012). The namespace
/// needs no privileged captures, so it installs with the host tables during
/// host injection, beside `messages`.
///
/// # Errors
/// Returns [`Error::Lua`] if the function or the global install fails.
pub(crate) fn install_compactors(lua: &Lua, globals: &Table) -> Result<()> {
    let fail: Function = lua
        .create_function(|_lua, tag: String| {
            let reason = OverflowReason::from_tag(&tag).ok_or_else(|| {
                mlua::Error::external(Error::Lua(format!(
                    "unknown overflow reason {tag:?}; expected \"precheck\" or \"provider\""
                )))
            })?;
            Err::<(), _>(mlua::Error::external(Error::ContextExhausted { reason }))
        })
        .map_err(Error::lua)?;
    let compactors = lua.create_table().map_err(Error::lua)?;
    compactors.raw_set("fail", fail).map_err(Error::lua)?;
    globals
        .raw_set("compactors", compactors)
        .map_err(Error::lua)
}

#[cfg(test)]
mod tests;
