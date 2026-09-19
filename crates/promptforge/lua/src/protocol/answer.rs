//! The answer vocabulary: one dispatched request's outcome and the payload
//! types its variants carry.

use promptforge_api_types::events::{CallMetrics, ToolCallEvent};

use crate::{Error, LuaFanoutResult, Result, ToolOutputKind};

/// The outcome of one dispatched store operation: the value the shim
/// returns to its caller. Mutating ops carry `Unit` (the shim returns
/// nil), exactly as the legacy closures returned nil.
#[derive(Debug)]
pub enum StoreOutcome {
    /// The operation succeeded with no return value.
    Unit,
    /// `read`/`read_numbered`: the (possibly bounded) file text.
    Text(String),
    /// `glob`: the matching paths, sorted.
    Paths(Vec<String>),
    /// `exists`: the presence flag.
    Bool(bool),
}

/// One dispatched `tools.call`'s successful output, classified by the
/// binding's declared [`ToolOutputKind`] so the envelope resumes the right
/// Lua shape: a plain binding's text resumes as a Lua string, a structured
/// binding's parsed JSON resumes as a Lua table through the serde boundary.
/// Scripts never see a JSON codec; the host performs the one conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallOutcome {
    /// A plain binding's output text, resumed as a Lua string - every
    /// existing tool, byte-identical to the tool loop's echo.
    Plain(String),
    /// A structured binding's parsed JSON output, resumed as a Lua table.
    Structured(serde_json::Value),
}

impl ToolCallOutcome {
    /// Classifies one dispatched tool's output text by the binding's
    /// declared output kind.
    ///
    /// Plain output passes through untouched. Structured output must parse
    /// as JSON - the untrusted nonce wrap is a string mechanism, so a
    /// structured binding whose output was wrapped fails here too, keeping
    /// structured output effectively restricted to trusted tools.
    ///
    /// # Errors
    /// Returns [`Error::Tool`] when a structured binding's output is not
    /// valid JSON, retaining the parse failure as the cause.
    pub fn from_dispatch(kind: ToolOutputKind, alias: &str, text: String) -> Result<Self> {
        match kind {
            ToolOutputKind::Plain => Ok(ToolCallOutcome::Plain(text)),
            ToolOutputKind::Structured => match serde_json::from_str(&text) {
                Ok(json) => Ok(ToolCallOutcome::Structured(json)),
                Err(error) => Err(Error::Tool {
                    message: format!("structured tool {alias:?} returned invalid JSON"),
                    source: Box::new(error),
                }),
            },
        }
    }
}

/// One completed `models.chat` round, resumed into the agent program as a
/// plain result table.
///
/// Exactly one of `reply` and `tool_calls` is present: the round produced
/// text or requested tools, never both. Agents branch on the presence of
/// `tool_calls`, never on `finish_reason` - backends routinely finish
/// tool-call rounds with `stop`. Absent optional fields are simply never
/// set on the resumed table, so they read back as nil.
// No `Eq`: `metrics` carries `f64` timings transitively.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResult {
    /// The completed reply text, when the round produced text.
    pub reply: Option<String>,
    /// The tool calls the model requested, unexecuted, when it requested
    /// any.
    pub tool_calls: Option<Vec<ToolCallEvent>>,
    /// The provider's finish reason, when it sent one.
    pub finish_reason: Option<String>,
    /// The model that served the round, as the response body named it
    /// (empty when the body named none).
    pub model: String,
    /// Everything measured about the round.
    pub metrics: Option<CallMetrics>,
}

/// The successful answer to a `user_input` request: the resumed text and
/// its availability flag.
///
/// `available` is `true` when `text` is the operator's own input and
/// `false` when the host had no input to give and `text` is the broker's
/// fixed fallback sentence. The flag rides beside the text - never encoded
/// into it - so a human typing exactly the fallback sentence cannot spoof
/// the unavailable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInputOutcome {
    /// The operator's text, or the fixed fallback sentence when
    /// `available` is `false`.
    pub text: String,
    /// Whether `text` is real operator input.
    pub available: bool,
}

/// One dispatched request's outcome, rendered to the `(ok, result)` envelope
/// at resume time.
///
/// The typed error is never flattened into the envelope: on failure the
/// envelope carries only the display string for the shim to raise, and
/// [`into_envelope`](Answer::into_envelope) hands the typed error back to the
/// driver, which retains it against the pending request and substitutes it
/// when the shim-raised error surfaces as the coroutine's failure. This holds
/// uniformly for leaf and structural answers: the enum owns the typed error
/// until the envelope is rendered, so a `Call` or `Fanout` failure
/// round-trips with its structure intact, never stringified.
///
/// The error type is the driver's: the Lua side produces
/// `Answer<`[`Error`]`>` (argument-validation failures at the yield
/// boundary), while the executor's scheduler drives `Answer` over its own
/// substrate so a dispatch failure (a gateway completion error, a binding
/// failure) round-trips typed.
#[derive(Debug)]
pub enum Answer<E> {
    /// The completion text for an `infer` request.
    Infer(std::result::Result<String, E>),
    /// The contained chain's final text for a `call` request.
    Call(std::result::Result<String, E>),
    /// The ordered arm results for a `fanout` request, in collection order.
    Fanout(std::result::Result<Vec<LuaFanoutResult>, E>),
    /// The classified output for a `chat` request. Boxed so the metrics-heavy
    /// [`ChatResult`] does not size every answer the non-chat paths move.
    Chat(std::result::Result<Box<ChatResult>, E>),
    /// The outcome of a `loop` request: the loop appends the history itself
    /// and returns nil, so success carries no value.
    Loop(std::result::Result<(), E>),
    /// The classified output for a `tools.call` request.
    ToolCallResult(std::result::Result<ToolCallOutcome, E>),
    /// The outcome of a `user_input` request: the resumed text and its
    /// availability flag.
    UserInput(std::result::Result<UserInputOutcome, E>),
    /// The outcome of a `store` request: the operation's return value.
    Store(std::result::Result<StoreOutcome, E>),
}

impl<E> Answer<E> {
    /// Maps the carried error type, leaving every success value untouched.
    pub fn map_error<F>(self, map: impl FnOnce(E) -> F) -> Answer<F> {
        match self {
            Answer::Infer(result) => Answer::Infer(result.map_err(map)),
            Answer::Call(result) => Answer::Call(result.map_err(map)),
            Answer::Fanout(result) => Answer::Fanout(result.map_err(map)),
            Answer::ToolCallResult(result) => Answer::ToolCallResult(result.map_err(map)),
            Answer::Chat(result) => Answer::Chat(result.map_err(map)),
            Answer::Loop(result) => Answer::Loop(result.map_err(map)),
            Answer::UserInput(result) => Answer::UserInput(result.map_err(map)),
            Answer::Store(result) => Answer::Store(result.map_err(map)),
        }
    }
}
