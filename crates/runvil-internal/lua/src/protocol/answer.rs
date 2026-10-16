//! The answer vocabulary: one dispatched request's outcome and the payload
//! types its variants hold.
//!
//! A local tool call is answered in two steps. The `tool_call` request's
//! answer is [`ToolCallOutcome::Local`], which resumes the shim with the
//! handler itself so the shim runs it inside the calling coroutine. The
//! shim then yields `local_tool_done` with the handler's result, and that
//! request's answer is an ordinary [`Answer::ToolCallResult`]: the text
//! on success, the error for a rejected return.

use runvil_types::ids::{TaskId, TaskOrigin};
use runvil_types::metrics::ToolCallEvent;

use crate::compactors::OverflowReason;
use crate::error::{Error, Result};
use crate::handles::ToolOutputKind;

/// The result of one [`VfsOp`](super::VfsOp) run against the run's store
/// view.
///
/// The Lua program receives it as the return value of its `store.*` call.
/// Operations that change the store, such as writes and deletes, produce
/// `Unit`, which the program receives as nil.
///
/// The run log records a successful store answer as this value, so its
/// JSON form is part of the log format. In serde's externally tagged form,
/// `Unit` is the bare string `"Unit"` and every other variant is a one-key
/// object, such as `{"Text": "..."}`, `{"Paths": ["a.txt"]}`, or
/// `{"Bool": true}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum VfsOutcome {
    /// The operation succeeded, and the program receives nil.
    Unit,
    /// The text that `read` or `read_numbered` returns: the whole file, or
    /// only the requested line range.
    Text(String),
    /// The paths that match a `glob` pattern, in sorted order.
    Paths(Vec<String>),
    /// Whether the path exists, for `exists`.
    Bool(bool),
}

/// One dispatched `tools.call`'s successful output, classified by the
/// binding's declared [`ToolOutputKind`] so the envelope resumes the right
/// Lua shape: a plain binding's text resumes as a Lua string, a structured
/// binding's parsed JSON resumes as a Lua table through the serde boundary.
/// The Engine performs the one conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallOutcome {
    /// A plain binding's output text, resumed as a Lua string - every
    /// existing tool, byte-identical to the tool loop's echo.
    Plain(String),
    /// A structured binding's parsed JSON output, resumed as a Lua table.
    Structured(serde_json::Value),
    /// A local Lua tool's call, resumed as `(true, nil, handler, args)`:
    /// the handler registered under `alias` and a fresh table converted
    /// from `args`, so the shim runs the handler inside the calling
    /// coroutine and reports how it ended with a `local_tool_done` yield.
    Local {
        /// The local tool's alias, naming its handler.
        alias: String,
        /// The call's JSON arguments.
        args: serde_json::Value,
    },
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
                    message: format!("structured tool {alias:?} returned invalid JSON: {error}"),
                    cause: Box::new(error),
                }),
            },
        }
    }
}

/// What one `chat` round produced, or that no round ran.
///
/// Callers branch on the variant, never on the round's finish reason
/// alone - backends routinely finish tool-call rounds with `stop`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatOutcome {
    /// The request was refused as too large before or by the provider:
    /// no round ran. The reason says which gate refused it, the
    /// pre-dispatch precheck or the provider; the loop's state machine
    /// (`models_loop`) hands its tag to the compactor.
    Overflow(OverflowReason),
    /// The round completed with reply text. A completion the caller builds
    /// can deliver an empty string, which the loop reads as no reply.
    Reply(String),
    /// The round completed requesting these tool calls, unexecuted.
    ToolCalls(Vec<ToolCallEvent>),
    /// The round completed with neither text nor tool calls, holding the
    /// client's fixed phrase naming the empty product when it named one.
    /// The loop's state machine (`models_loop`) applies its exit rules
    /// against the finish reason, and raises the phrase as the
    /// `empty_model_reply` message when they reject the round, so the
    /// author sees the text the client would have produced.
    Empty(Option<String>),
}

/// One `chat` round's result, resumed into the loop shim as an opaque
/// value: a userdata with no methods, which the loop's step takes back out
/// whole, so no Lua code reads its fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResult {
    /// What the round produced, or that no round ran.
    pub outcome: ChatOutcome,
    /// The provider's finish reason, when it sent one; absent when no
    /// round ran.
    pub finish_reason: Option<String>,
    /// The turn the round was reported under: the advanced counter for a
    /// served or empty round, the unchanged counter when no round ran.
    /// The loop shim passes it back with each requested tool call, so the
    /// call reports the round that requested it.
    pub turn: u32,
}

impl mlua::UserData for ChatResult {}

/// One task's delivery to a `join_any` waiter: which member ended and how.
///
/// `outcome` is the task's final text, or its failure as the error value
/// the shim hands back (`ok = false`): the task chain's own error, or the
/// `cancelled` value for a task that was cancelled or abandoned. The
/// delivery itself succeeded; a wait that fails outright (a task the
/// caller does not own, a result already delivered) is the outer
/// [`Answer::JoinAny`] error instead. A delivered failure is also the
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

/// A task's lifecycle phase, as `tasks.status` reports it in `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    /// The task's backing chain or request is still running.
    Running,
    /// The task ended with a result, whether or not a wait delivered it.
    Done,
    /// The owner cancelled the task.
    Cancelled,
    /// The task's owner ended while the task was live.
    Abandoned,
}

impl TaskPhase {
    /// The tag `tasks.status` reports: `running`, `done`, `cancelled`, or
    /// `abandoned`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            TaskPhase::Running => "running",
            TaskPhase::Done => "done",
            TaskPhase::Cancelled => "cancelled",
            TaskPhase::Abandoned => "abandoned",
        }
    }
}

/// What a task's live backing chain is waiting on, as `tasks.status`
/// reports it in `blocked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// A `chat` round.
    Chat,
    /// A tool call.
    ToolCall,
    /// A store operation.
    Store,
    /// A task wait.
    Tasks,
    /// A `call` chain.
    Call,
    /// A concurrency slot, before admission.
    Queued,
}

impl Blocked {
    /// The tag `tasks.status` reports: `chat`, `tool_call`, `store`,
    /// `tasks`, `call`, or `queued`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Blocked::Chat => "chat",
            Blocked::ToolCall => "tool_call",
            Blocked::Store => "store",
            Blocked::Tasks => "tasks",
            Blocked::Call => "call",
            Blocked::Queued => "queued",
        }
    }
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
    /// The lifecycle phase; a delivered task reports [`TaskPhase::Done`].
    pub state: TaskPhase,
    /// Whether the task ended well: `None` while it runs, `Some(false)`
    /// for a failed, cancelled, or abandoned task.
    pub ok: Option<bool>,
    /// The section the backing chain is currently in, while it is live and
    /// inside one.
    pub section: Option<String>,
    /// What the backing chain is waiting on, while it waits.
    pub blocked: Option<Blocked>,
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
/// On failure the envelope holds the error's structured table for the
/// shim to raise, and the driver
/// ([`SectionVm::resume_block_coro_answer`](crate::SectionVm::resume_block_coro_answer))
/// retains the typed error beside that very table. When the block fails
/// by raising that table, the shim's own raise or the block's re-raise of
/// the value it caught, the typed error is substituted back; any other
/// failure, one carrying the same message included, stays the block's
/// own. This holds uniformly for leaf and structural answers: the enum
/// owns the typed error until the envelope is rendered, so a `Call` or
/// `JoinAny` failure round-trips with its structure intact.
///
/// The error type is the driver's: the Lua side produces
/// `Answer<`[`Error`]`>` (argument-validation failures at the yield
/// boundary), while the executor's scheduler drives `Answer` over its own
/// error type so a dispatch failure (a gateway completion error, a binding
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
    /// The member delivered for a `join_any` request.
    JoinAny(std::result::Result<TaskDelivery<E>, E>),
    /// Whether the task has ended, for a `ready` request.
    Ready(std::result::Result<bool, E>),
    /// The task's status table for a `status` request. Boxed so the
    /// field-heavy [`TaskStatus`] does not size every answer.
    Status(std::result::Result<Box<TaskStatus>, E>),
    /// The caller's live tasks in spawn order, for a `pending` request;
    /// the shim wraps each id in a `Task` handle.
    Pending(std::result::Result<Vec<TaskId>, E>),
    /// The chain's effective admission limit, for a `concurrency`
    /// request: the setter's clamped value, or the current one when the
    /// call gave no argument.
    Concurrency(std::result::Result<usize, E>),
    /// The unit outcome of a `note` request.
    Note(std::result::Result<(), E>),
    /// The unit outcome of a `cancel` request.
    Cancel(std::result::Result<(), E>),
    /// The classified output for a `chat` request. Boxed so the field-heavy
    /// [`ChatResult`] does not size every answer the non-chat paths move.
    Chat(std::result::Result<Box<ChatResult>, E>),
    /// The classified output for a `tools.call` request.
    ToolCallResult(std::result::Result<ToolCallOutcome, E>),
    /// The texts of a `tool_calls` request's issued calls in call order,
    /// or the first failure in call order. Fewer texts than calls means
    /// the scheduler stopped at the call after the last one answered.
    ToolCallsResult(std::result::Result<Vec<String>, E>),
    /// The outcome of a `store` request: the operation's return value.
    Store(std::result::Result<VfsOutcome, E>),
}

impl<E> Answer<E> {
    /// Maps the error type, leaving every success value untouched.
    pub fn map_error<F>(self, map: impl FnOnce(E) -> F) -> Answer<F> {
        match self {
            Answer::Infer(result) => Answer::Infer(result.map_err(map)),
            Answer::Call(result) => Answer::Call(result.map_err(map)),
            Answer::Spawn(result) => Answer::Spawn(result.map_err(map)),
            Answer::Timer(result) => Answer::Timer(result.map_err(map)),
            Answer::JoinAny(result) => Answer::JoinAny(match result {
                Ok(TaskDelivery { task, outcome }) => Ok(TaskDelivery {
                    task,
                    outcome: outcome.map_err(map),
                }),
                Err(error) => Err(map(error)),
            }),
            Answer::Ready(result) => Answer::Ready(result.map_err(map)),
            Answer::Status(result) => Answer::Status(result.map_err(map)),
            Answer::Pending(result) => Answer::Pending(result.map_err(map)),
            Answer::Concurrency(result) => Answer::Concurrency(result.map_err(map)),
            Answer::Note(result) => Answer::Note(result.map_err(map)),
            Answer::Cancel(result) => Answer::Cancel(result.map_err(map)),
            Answer::ToolCallResult(result) => Answer::ToolCallResult(result.map_err(map)),
            Answer::ToolCallsResult(result) => Answer::ToolCallsResult(result.map_err(map)),
            Answer::Chat(result) => Answer::Chat(result.map_err(map)),
            Answer::Store(result) => Answer::Store(result.map_err(map)),
        }
    }
}
