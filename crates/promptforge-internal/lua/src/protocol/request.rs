//! The request vocabulary: the validated suspending Engine calls a shim can
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
use crate::messages::MessageList;

/// A validated suspending Engine call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: a
/// spawn's `item` seed converts through the serde bridge and the handle
/// userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
/// lifetime-bound enters the enum.
#[derive(Debug)]
pub enum Request {
    /// `models.infer(prompt)` (`binding: None`: resolve the section's
    /// current model) or `h:infer(prompt)` (`binding: Some`: the handle's
    /// frozen binding).
    Infer {
        /// The author-supplied prompt text.
        prompt: String,
        /// The receiver handle's frozen binding, else `None`.
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
    /// `tasks.join_any(set)`: park the chain until the first task in `set`
    /// ends, or resume at once when one already has. The one scheduler
    /// wait primitive: `tasks.join` is Lua over it.
    JoinAny {
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
    /// `tasks.concurrency(limit?)`: set the chain's admission limit for
    /// the tasks it spawns from here on, or read the effective limit back
    /// with no argument. The limit is shim-validated (a positive whole
    /// number) before the yield, so a present `limit` is always one.
    Concurrency {
        /// The author-supplied limit, absent for the read-only form.
        limit: Option<u64>,
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
    /// One stateless tool-capable model round over a `messages.new()`
    /// list, yielded by the `models.loop` shim once per round. The
    /// dispatch arm first pushes the chain's undelivered model-task
    /// notices onto the list as user records and refuses a list that is
    /// still empty; the round advertises the section's current tool
    /// scope, local Lua tools included, resolved in the dispatch arm
    /// where the tool scope sits.
    Chat {
        /// The list the round sends, which may be empty, since the
        /// dispatch pushes pending notices before it refuses an empty
        /// list. Its records were validated as the list added them.
        list: MessageList,
        /// The receiver of a handle's `loop`, as its frozen binding cloned
        /// out of the userdata while the VM handle is live; `None` when
        /// the round names no handle, and the driver resolves the
        /// section's current model.
        binding: Option<ModelBinding>,
    },
    /// `store.*(...)`: one run-scoped store operation as a leaf yield.
    /// Section VMs and the live H1 VM run the store shims. Every operation
    /// takes this path uniformly - memory- and real-file-backed alike, with no
    /// inline fast path - so interleaving behavior never depends on the
    /// backend.
    Store {
        /// The validated operation and its author-supplied arguments.
        op: VfsOp,
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

/// One `store.*` call from a prompt script: which operation it is and the
/// arguments the script passed.
///
/// The Engine checks the arguments once, when it parses the call.
///
/// This type carries the full arguments of one of the eight `store.*`
/// calls. The separate `vfs::Op` names only the kind of file operation
/// (`Read`, `Write`, `Rename`, and so on), which a policy matches on and a
/// watcher is told about.
///
/// The read bounds are `i64`. A negative bound converts to 0 when the
/// operation runs, so the range check rejects it with the same error a
/// zero bound produces.
///
/// The type holds only plain data, so an effect record can carry an
/// operation through serde intact.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum VfsOp {
    /// `store.write(path, contents)`: creates or overwrites the file at
    /// `path` with `contents`.
    Write {
        /// The author-supplied logical path.
        path: String,
        /// The author-supplied file contents.
        contents: String,
    },
    /// `store.append(path, contents)`: appends `contents` to the file at
    /// `path`, creating the file if it is absent.
    Append {
        /// The author-supplied logical path.
        path: String,
        /// The author-supplied text to append.
        contents: String,
    },
    /// `store.read(path, start?, end?)`: reads the file at `path` as text.
    ///
    /// With both bounds omitted, it reads the whole file. With `start`, it
    /// reads a 1-based inclusive line range. An `end` given alone is an
    /// error.
    Read {
        /// The author-supplied logical path.
        path: String,
        /// The optional 1-based first line.
        start: Option<i64>,
        /// The optional 1-based last line.
        end: Option<i64>,
    },
    /// `store.read_numbered(path, start?, end?)`: reads like `store.read`
    /// and prefixes each line with its absolute line number. The bounds
    /// work the same way.
    ReadNumbered {
        /// The author-supplied logical path.
        path: String,
        /// The optional 1-based first line.
        start: Option<i64>,
        /// The optional 1-based last line.
        end: Option<i64>,
    },
    /// `store.str_replace(path, old, new)`: replaces `old` with `new` in the
    /// file at `path`.
    StrReplace {
        /// The author-supplied logical path.
        path: String,
        /// The anchor text, required to occur exactly once.
        old: String,
        /// The replacement text.
        new: String,
    },
    /// `store.delete(path)`: deletes the file at `path`. Deleting a missing
    /// path succeeds.
    Delete {
        /// The author-supplied logical path.
        path: String,
    },
    /// `store.glob(pattern)`: lists the paths that match `pattern`.
    Glob {
        /// The author-supplied glob pattern.
        pattern: String,
    },
    /// `store.exists(path)`: reports whether `path` exists.
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

impl MessageRecord {
    /// The record's JSON form: `role`, `content`, and `tool_calls` and
    /// `tool_call_id` when present, in the shapes the chat parse accepts,
    /// so the parse turns it back into an equal record.
    #[must_use]
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        entry.insert("role".to_owned(), self.role.as_str().into());
        entry.insert("content".to_owned(), self.content.to_json());
        if let Some(calls) = self.tool_calls_json() {
            entry.insert("tool_calls".to_owned(), calls);
        }
        if let Some(id) = &self.tool_call_id {
            entry.insert("tool_call_id".to_owned(), id.as_str().into());
        }
        serde_json::Value::Object(entry)
    }

    /// The `tool_calls` field's JSON form: an array of `{id, name,
    /// arguments}` objects, or `None` when the record holds no calls.
    pub(crate) fn tool_calls_json(&self) -> Option<serde_json::Value> {
        (!self.tool_calls.is_empty()).then(|| {
            self.tool_calls
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "id": call.id,
                        "name": call.name,
                        "arguments": call.arguments,
                    })
                })
                .collect()
        })
    }
}

impl MessageContent {
    /// The `content` field's JSON form: the text, or an array of `text`
    /// and `image_url` parts.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            MessageContent::Text(text) => text.as_str().into(),
            MessageContent::Parts(parts) => parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text(text) => serde_json::json!({ "type": "text", "text": text }),
                    ContentPart::ImageUrl(url) => {
                        serde_json::json!({ "type": "image_url", "image_url": { "url": url } })
                    }
                })
                .collect(),
        }
    }
}
