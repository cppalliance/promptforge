//! The answer vocabulary: one dispatched request's outcome and the payload
//! types its variants carry.

use promptforge_api_types::events::{CallMetrics, ToolCallEvent};
use promptforge_api_types::ids::{TaskId, TaskOrigin};

use crate::compactors::OverflowReason;
use crate::{Error, Result, ToolOutputKind};

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

/// One `chat` round's outcome, resumed into the program as a plain result
/// table.
///
/// When `overflow` is set the request was refused as too large before or
/// by the provider: no round ran, `overflow_reason` says which of the two
/// refused it, and every other field is absent or empty. Otherwise the
/// round completed and at most one of `reply` and `tool_calls` is present:
/// the round produced text or requested tools, never both. An empty reply
/// is a completed round with `reply` absent (never an empty string) and
/// `empty_detail` naming the empty product, so the loop shim applies its
/// exit rules against `finish_reason`. Callers branch on the presence of
/// `tool_calls` and `reply`, never on `finish_reason` alone - backends
/// routinely finish tool-call rounds with `stop`. Absent optional fields
/// are simply never set on the resumed table, so they read back as nil;
/// `overflow` is always set, as a boolean.
// No `Eq`: `metrics` carries `f64` timings transitively.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResult {
    /// Whether the request was refused as too large before or by the
    /// provider. No round ran; the loop shim invokes the compactor.
    pub overflow: bool,
    /// Which gate refused the request when `overflow` is set: the
    /// pre-dispatch precheck or the provider. The loop shim hands its tag
    /// to the compactor.
    pub overflow_reason: Option<OverflowReason>,
    /// The completed reply text, when the round produced non-empty text.
    pub reply: Option<String>,
    /// The client's fixed phrase naming the empty product, when the round
    /// completed with neither text nor tool calls. The loop shim raises it
    /// as the `empty_model_reply` message when its exit rules reject the
    /// round, so the author sees the text the client would have produced.
    pub empty_detail: Option<String>,
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

/// One task's delivery to a `when_any` waiter: which member ended and how.
///
/// `outcome` is the task's final text, or its failure as the error value
/// the shim hands back (`ok = false`): the task chain's own error, or the
/// `cancelled` value for a task that was cancelled or abandoned. The
/// delivery itself succeeded; a wait that fails outright (a task the
/// caller does not own, a result already delivered) is the outer
/// [`Answer::WhenAny`] error instead. A delivered failure is also the
/// envelope's retained typed error, so a shim that re-raises the member's
/// failure at once (the `fanout` shim's fatal-arm path) hands the driver
/// the member's own typed error rather than its rendering.
#[derive(Debug)]
pub struct TaskDelivery<E> {
    /// The member that ended.
    pub task: TaskId,
    /// The member's final text or failure.
    pub outcome: std::result::Result<String, E>,
}

/// One task's status, as `tasks.status` reports it: the slot's facts plus
/// a live backing chain's position. Every optional field resumes as nil
/// when absent, so an author tests presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    /// The name of the section the task's chain started at.
    pub target: String,
    /// The principal that started the task.
    pub origin: TaskOrigin,
    /// The lifecycle state tag: `running`, `done`, `cancelled`, or
    /// `abandoned` (a delivered task reports `done`).
    pub state: &'static str,
    /// Whether the task ended well: `None` while it runs, `Some(false)`
    /// for a failed, cancelled, or abandoned task.
    pub ok: Option<bool>,
    /// The section the backing chain is currently in, while it is live and
    /// inside one.
    pub section: Option<String>,
    /// What the backing chain is parked on (`chat`, `tool_call`,
    /// `user_input`, `store`, `timer`, `tasks`, `call`), while it is.
    pub blocked: Option<&'static str>,
    /// The task's model-turn count so far.
    pub turns: u32,
    /// The live tasks the task's chain owns, in spawn order.
    pub tasks: Vec<TaskId>,
    /// The task chain's call depth.
    pub depth: u32,
    /// The latest note the task published through `tasks.note`.
    pub note: Option<String>,
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
/// until the envelope is rendered, so a `Call` or `WhenAny` failure
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
    /// The started task's id for a `spawn` request, resumed as its path
    /// text; the shim wraps it in the methodless `Task` table.
    Spawn(std::result::Result<TaskId, E>),
    /// The started timer's task id for a `timer` request, resumed as its
    /// path text; the wait shim keeps it to wait on and cancel.
    Timer(std::result::Result<TaskId, E>),
    /// The member delivered for a `when_any` request.
    WhenAny(std::result::Result<TaskDelivery<E>, E>),
    /// Whether the task has ended, for a `ready` request.
    Ready(std::result::Result<bool, E>),
    /// The task's status table for a `status` request. Boxed so the
    /// field-heavy [`TaskStatus`] does not size every answer.
    Status(std::result::Result<Box<TaskStatus>, E>),
    /// The caller's live tasks in spawn order, for a `pending` request;
    /// the shim wraps each id in a `Task` handle.
    Pending(std::result::Result<Vec<TaskId>, E>),
    /// The unit outcome of a `note` request.
    Note(std::result::Result<(), E>),
    /// The unit outcome of a `cancel` request.
    Cancel(std::result::Result<(), E>),
    /// The chain's undelivered model-task notices in arrival order, for a
    /// `drain_task_notices` request; the shim appends each as a message
    /// record. Empty when nothing ended since the last drain.
    DrainTaskNotices(std::result::Result<Vec<String>, E>),
    /// The classified output for a `chat` request. Boxed so the metrics-heavy
    /// [`ChatResult`] does not size every answer the non-chat paths move.
    Chat(std::result::Result<Box<ChatResult>, E>),
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
            Answer::Spawn(result) => Answer::Spawn(result.map_err(map)),
            Answer::Timer(result) => Answer::Timer(result.map_err(map)),
            Answer::WhenAny(result) => Answer::WhenAny(match result {
                Ok(TaskDelivery { task, outcome }) => Ok(TaskDelivery {
                    task,
                    outcome: outcome.map_err(map),
                }),
                Err(error) => Err(map(error)),
            }),
            Answer::Ready(result) => Answer::Ready(result.map_err(map)),
            Answer::Status(result) => Answer::Status(result.map_err(map)),
            Answer::Pending(result) => Answer::Pending(result.map_err(map)),
            Answer::Note(result) => Answer::Note(result.map_err(map)),
            Answer::Cancel(result) => Answer::Cancel(result.map_err(map)),
            Answer::DrainTaskNotices(result) => Answer::DrainTaskNotices(result.map_err(map)),
            Answer::ToolCallResult(result) => Answer::ToolCallResult(result.map_err(map)),
            Answer::Chat(result) => Answer::Chat(result.map_err(map)),
            Answer::UserInput(result) => Answer::UserInput(result.map_err(map)),
            Answer::Store(result) => Answer::Store(result.map_err(map)),
        }
    }
}
