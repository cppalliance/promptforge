//! The request vocabulary: the validated suspending host calls a shim can
//! yield, the store operations they hold, and the message-record types
//! the chat request is built from.
//!
//! A local tool call spans two requests. The first is the ordinary
//! [`Request::ToolCall`], which the scheduler answers with the handler.
//! The shim runs the handler inside the calling coroutine, so any
//! suspending call the handler makes is a request of its own, and then
//! yields [`Request::LocalToolDone`] with how the handler ended, so the
//! scheduler can report the call and answer it.

use promptforge_model_client::model::ModelBinding;
use promptforge_types::ids::{TaskId, TaskOrigin};

use crate::Error;

/// A validated suspending host call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: a
/// spawn's `item` seed converts through the serde bridge and the handle
/// userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
/// lifetime-bound enters the enum.
#[derive(Debug)]
pub enum Request {
    /// `models.infer(prompt)` (`binding: None`: resolve the section's
    /// current model) or `models.infer(handle, prompt)` (`binding: Some`:
    /// the handle's frozen binding).
    Infer {
        /// The author-supplied prompt text.
        prompt: String,
        /// The leading handle's frozen binding, else `None`.
        binding: Option<ModelBinding>,
    },
    /// `call(target, input?)`: run a contained chain over the target's
    /// slice.
    Call {
        /// The heading string, validated with the `resolve_section_target`
        /// rule so a non-string target keeps its byte-identical error.
        target: String,
        /// The optional input override; `None` runs under the run's own args.
        input: Option<String>,
        /// The caller's `var` snapshot, seeded into the chain and discarded
        /// when it ends.
        var: serde_json::Value,
    },
    /// `tasks.spawn(target, opts?)`, and each arm of the `fanout` shim:
    /// start a task chain over the target's slice and return at once. The
    /// chain shares `call`'s target resolution and depth cap, and refuses a
    /// list section as the target (the worker-template check).
    Spawn {
        /// The heading string, validated with the `resolve_section_target`
        /// rule so a non-string target keeps its byte-identical error.
        target: String,
        /// `opts.input`: the chain's `args` override; `None` runs under the
        /// caller's own args.
        input: Option<String>,
        /// `opts.item`: the chain's `item` global and `{{ item }}` seed;
        /// `None` installs no `item`.
        item: Option<serde_json::Value>,
        /// `opts.index`: the chain's `sys.index`; `None` leaves the field
        /// absent, as outside a fanout.
        index: Option<u64>,
        /// The caller's `var` snapshot, seeded into the chain and discarded
        /// when it ends.
        var: serde_json::Value,
        /// The principal starting the task. Shim-produced, never
        /// author-supplied: the author shim always says `author`.
        origin: TaskOrigin,
        /// Whether the spawn is a `fanout` arm. Shim-produced: the `fanout`
        /// shim says `true`, `tasks.spawn` leaves it absent. The depth-cap
        /// refusal is named after the author-facing call that tripped it
        /// (`fanout` or `call`), so the typed error uses the wording the
        /// author reads and no shim re-match is needed.
        fanout: bool,
    },
    /// The wait shims' internal timeout timer: a leaf request whose work
    /// is one sleep, registered as an effect-backed task slot the caller
    /// owns and resumed at once with the slot's id, so the shim can wait
    /// on it beside the members and cancel it when a member wins. Never
    /// author-visible: the shim yields it for `opts.timeout` and keeps
    /// the id.
    Timer {
        /// `opts.timeout`: how long the timer runs before it fires, in
        /// seconds. Non-negative, finite, and within `Duration`'s range by
        /// the parse.
        seconds: f64,
    },
    /// `tasks.when_any(set)`: park the chain until the first task in `set`
    /// ends, or resume at once when one already has. The one scheduler
    /// wait primitive: `tasks.when_all` is Lua over it.
    WhenAny {
        /// The tasks to wait on, in the author's order: the first terminal
        /// member in this order is the one delivered when several are.
        /// Non-empty by the shim's check.
        tasks: Vec<TaskId>,
    },
    /// `tasks.ready(task)`: the non-blocking check whether `task` has
    /// ended (in any terminal state, delivered or not).
    Ready {
        /// The task to inspect.
        task: TaskId,
    },
    /// `tasks.status(task)`: the task's status table. Owner-or-self: the
    /// caller may inspect a task it owns or the task it runs inside.
    Status {
        /// The task to inspect.
        task: TaskId,
    },
    /// `tasks.pending(filter?)`: the caller's live tasks in spawn order,
    /// optionally narrowed to one origin.
    Pending {
        /// `filter.origin`, when given.
        origin: Option<TaskOrigin>,
    },
    /// `tasks.note(text)`: publish the caller's own task's latest progress
    /// note, read back by `tasks.status`.
    Note {
        /// The author-supplied note text.
        text: String,
    },
    /// `tasks.cancel(task)`: end a task the caller owns. Idempotent: a
    /// task already in a terminal state is left as it is.
    Cancel {
        /// The task to cancel.
        task: TaskId,
    },
    /// `tasks.events(task, opts?)`: the events one task has reported so
    /// far, read from the host's history. Owner-or-self, as `status` is: the
    /// caller may read a task it owns or the task it runs inside. A leaf
    /// request: the host answers it from its log (a test driver from its
    /// event buffer).
    TaskEvents {
        /// The task whose events are read.
        task: TaskId,
        /// `opts.last`: the highest task sequence number the caller has
        /// already seen; only events after it are returned. `None` reads
        /// from the task's start.
        last: Option<u32>,
    },
    /// The loop shim's per-round drain of the chain's undelivered
    /// model-task notices: the engine's sentences telling the model how
    /// the tasks it started ended, answered at once in arrival order and
    /// appended to the author's message list ahead of the round's `chat`.
    /// Shim-produced and argument-free: the shim yields it for every
    /// round, so a chain with no model tasks drains an empty list.
    DrainTaskNotices,
    /// `tools.call(alias_or_tool, args)`: suspending dispatch of a bound
    /// tool through the shared dispatch function.
    ToolCall {
        /// The author-supplied prompt-local tool alias.
        alias: String,
        /// The author-supplied JSON arguments; an absent or nil `args`
        /// parses as the empty object.
        args: serde_json::Value,
        /// `Some`: a model-issued call, set by the loop shim from the
        /// model's tool call. It always resumes with content (a tool's own
        /// failure becomes untrusted failure text) and `ToolResult` fires
        /// under this id. `None`: a script call, which keeps the
        /// raise-at-call-site behavior. Shim-produced, never
        /// author-supplied: a wrong shape is a malformed yield.
        call_id: Option<String>,
        /// `Some`: the turn of the chat round that requested a
        /// model-issued call, passed back by the loop shim from the
        /// round's result. `None`: a script call, or the test-only
        /// `tools.call_as_model` hook, which report the live counter.
        /// Shim-produced, never author-supplied: a wrong shape is a
        /// malformed yield.
        turn: Option<u32>,
    },
    /// The second yield of a local tool call: the shim ran the handler the
    /// `tool_call` answer handed it and reports how the handler ended.
    /// Shim-produced in every field, so a wrong shape is a malformed
    /// yield, never an error at the author's call site.
    LocalToolDone {
        /// How the handler ended.
        outcome: LocalToolOutcome,
    },
    /// One stateless tool-capable model round over an author-built message
    /// list, yielded by the `models.loop` shim once per round. The round
    /// advertises the section's current tool scope, local Lua tools
    /// included, resolved in the dispatch arm where the tool scope sits.
    Chat {
        /// The validated message records. Each holds a known role
        /// ([`MessageRole`]), visible text or a non-empty content-parts
        /// array ([`MessageContent`]), the normalized tool calls an
        /// assistant record requested, and the call ID a tool result
        /// answers. Validation happens here, in the protocol parse, once -
        /// the driver converts without re-checking.
        messages: Vec<MessageRecord>,
        /// The loop shim's leading handle, as its frozen binding cloned
        /// out of the userdata while the VM handle is live; `None` when
        /// the round names no handle, and the driver resolves the
        /// section's current model.
        binding: Option<ModelBinding>,
    },
    /// `user_input()`: a direct operator-input request to the run's input
    /// broker. The request is argument-free: the broker and its host
    /// policy own the whole interaction.
    UserInput,
    /// `store.*(...)`: one run-scoped store operation as a leaf yield.
    /// Section VMs and the live H1 VM run the store shims. Every operation
    /// takes this path uniformly - memory- and host-backed alike, with no
    /// inline fast path - so interleaving behavior never depends on the
    /// backend.
    Store {
        /// The validated operation and its author-supplied arguments.
        op: StoreOp,
    },
    /// Reserved. Never dispatched: receiving one is a typed protocol error.
    Mcp {
        /// The reserved server name.
        server: String,
        /// The reserved tool name.
        tool: String,
        /// The reserved argument payload.
        args: serde_json::Value,
    },
}

/// How a local tool's handler ended, as its `local_tool_done` yield
/// reports it.
#[derive(Debug)]
pub enum LocalToolOutcome {
    /// The handler returned: its first return value as text under the
    /// scalar-return rule, `""` for nil.
    Returned(String),
    /// The handler returned a value with no text form (a table, a
    /// function): the scalar-return rule's error.
    BadReturn(Error),
    /// The handler raised. The shim raises the handler's own value again
    /// at the call site, so no error crosses the boundary here.
    Raised,
}

impl Request {
    /// The typed protocol error for a received `mcp` request.
    ///
    /// The `mcp` fields are reserved and no call surface produces the request
    /// yet, so the driver never dispatches one; receiving it fails the chain
    /// with this error rather than reaching an unimplemented path.
    #[must_use]
    pub fn mcp_reserved() -> Error {
        Error::Lua("mcp requests are reserved: no dispatcher exists yet".to_owned())
    }
}

/// One validated store operation: the `store.*` call's name and its
/// author-supplied arguments, checked once here at the protocol boundary.
///
/// The read bounds stay `i64` as the legacy callback's signature had
/// them: a negative bound converts to 0 at execution, which the facade's
/// range validation rejects with the same error a zero bound produces.
///
/// Plain data, so the executor's effect record can move an operation
/// through serde as the shim yielded it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum StoreOp {
    /// `store.write(path, contents)`.
    Write {
        /// The author-supplied logical path.
        path: String,
        /// The author-supplied file contents.
        contents: String,
    },
    /// `store.append(path, contents)`.
    Append {
        /// The author-supplied logical path.
        path: String,
        /// The author-supplied text to append.
        contents: String,
    },
    /// `store.read(path, start?, end?)`: no `start` reads the whole file;
    /// a present `start` slices a 1-based inclusive line range.
    Read {
        /// The author-supplied logical path.
        path: String,
        /// The optional 1-based first line.
        start: Option<i64>,
        /// The optional 1-based last line.
        end: Option<i64>,
    },
    /// `store.read_numbered(path, start?, end?)`: the read with absolute
    /// line numbers under the same optional bounds.
    ReadNumbered {
        /// The author-supplied logical path.
        path: String,
        /// The optional 1-based first line.
        start: Option<i64>,
        /// The optional 1-based last line.
        end: Option<i64>,
    },
    /// `store.str_replace(path, old, new)`.
    StrReplace {
        /// The author-supplied logical path.
        path: String,
        /// The anchor text, required to occur exactly once.
        old: String,
        /// The replacement text.
        new: String,
    },
    /// `store.delete(path)` (idempotent).
    Delete {
        /// The author-supplied logical path.
        path: String,
    },
    /// `store.glob(pattern)`.
    Glob {
        /// The author-supplied glob pattern.
        pattern: String,
    },
    /// `store.exists(path)`.
    Exists {
        /// The author-supplied logical path.
        path: String,
    },
}

/// The role of one validated message record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// System framing for the conversation.
    System,
    /// User input.
    User,
    /// Assistant output, with or without requested tool calls.
    Assistant,
    /// A tool result answering one assistant tool call.
    Tool,
}

impl MessageRole {
    /// Parses an author-facing role string; `None` for anything outside the
    /// four accepted roles.
    pub(super) fn parse(role: &str) -> Option<MessageRole> {
        match role {
            "system" => Some(MessageRole::System),
            "user" => Some(MessageRole::User),
            "assistant" => Some(MessageRole::Assistant),
            "tool" => Some(MessageRole::Tool),
            _ => None,
        }
    }

    /// The wire role string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

/// One content part of a multimodal message: visible text or a data-URI
/// image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    /// Visible text.
    Text(String),
    /// A data-URI image reference.
    ImageUrl(String),
}

/// A message record's content: plain visible text, or a non-empty
/// content-parts array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContent {
    /// Plain visible text.
    Text(String),
    /// A non-empty multimodal content-parts array.
    Parts(Vec<ContentPart>),
}

/// One normalized tool call an assistant message holds: the
/// provider-neutral `{id, name, arguments}` record every later component
/// consumes. The record stays neutral; the projection's `wire_message`
/// renders it as the OpenAI function-call wire shape at dispatch time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// The call identifier tool results correlate against.
    pub id: String,
    /// The wire name of the tool the model asked for.
    pub name: String,
    /// The call arguments; always an object, normalized to `{}` when the
    /// record had none.
    pub arguments: serde_json::Value,
}

/// One validated message record: the plain-message contract every later
/// component (projection, `models.loop`, the message builders) consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRecord {
    /// The message role.
    pub role: MessageRole,
    /// The visible content.
    pub content: MessageContent,
    /// The normalized tool calls the record holds; empty unless an
    /// assistant turn requested tools.
    pub tool_calls: Vec<ToolCallRecord>,
    /// The call ID a tool result answers; required on `tool` records.
    pub tool_call_id: Option<String>,
}
