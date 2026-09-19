//! The request vocabulary: the validated suspending host calls a shim can
//! yield, the store operations they carry, and the message-record types
//! the chat and loop requests are built from.

use promptforge_model_client::model::ModelBinding;

use crate::Error;

/// A validated suspending host call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: the
/// fanout collection converts through the existing member-wise rules and the
/// handle userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
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
    /// `fanout(worker, collection)`: the collection already converted
    /// member-wise through the existing rules.
    Fanout {
        /// The worker heading string, resolved by the driver against the
        /// caller's visible set.
        worker: String,
        /// The converted collection members: the array part in order, then
        /// the hash part as `{"key", "value"}` pairs.
        items: Vec<serde_json::Value>,
        /// The caller's `var` snapshot; each arm seeds from its own clone.
        var: serde_json::Value,
    },
    /// `tools.call(alias_or_tool, args)`: suspending dispatch of a bound
    /// tool through the shared dispatch function.
    ToolCall {
        /// The author-supplied prompt-local tool alias.
        alias: String,
        /// The author-supplied JSON arguments; an absent or nil `args`
        /// parses as the empty object.
        args: serde_json::Value,
    },
    /// `models.chat(messages, opts)`: one stateless tool-capable model
    /// round over an agent-built message list. Agent VMs alone install the
    /// shim; core's scheduler carries an unreachable internal-invariant
    /// guard for the arm its exhaustive match forces.
    Chat {
        /// The validated message records. Each carries a known role
        /// ([`MessageRole`]), visible text or a non-empty content-parts
        /// array ([`MessageContent`]), the normalized tool calls an
        /// assistant record requested, and the call ID a tool result
        /// answers. Validation lives here, in the protocol parse, once -
        /// the driver converts without re-checking.
        messages: Vec<MessageRecord>,
        /// `opts.model`: the catalog model to use for this round, or
        /// `None` for the program's current `models.use` selection.
        model: Option<String>,
        /// `opts.tools`: the tool aliases to advertise for exactly this
        /// round. Defaults to none; the driver never adds to it.
        tools: Vec<String>,
    },
    /// `models.loop(handle?, messages, compactor?)`: the Rust-backed
    /// model-tool loop over an author-owned message list. Section VMs alone
    /// install the shim; the agent driver carries an unreachable
    /// internal-invariant guard for the arm its exhaustive match forces.
    Loop {
        /// The validated message records, parsed once here exactly as for
        /// [`Request::Chat`]. The driver projects them per dispatch and
        /// appends every assistant message and correlated tool result to
        /// the author's list behind `messages_key`.
        messages: Vec<MessageRecord>,
        /// The registry key for the author's message list, stashed while
        /// the VM handle is live so the driver can append the loop's
        /// records to the very table the author passed.
        messages_key: mlua::RegistryKey,
        /// The leading handle's frozen binding, else `None` (the driver
        /// resolves the section's current model at call time).
        binding: Option<ModelBinding>,
        /// The registry key for the author-selected compactor callback,
        /// else `None` (the omitted-compactor default, `compactors.fail`).
        compactor: Option<mlua::RegistryKey>,
    },
    /// `user_input()`: a direct operator-input request to the run's input
    /// broker. Section VMs alone install the shim; the agent driver
    /// carries an unreachable internal-invariant guard for the arm its
    /// exhaustive match forces. The request carries no arguments: the
    /// broker and its host policy own the whole interaction.
    UserInput,
    /// `store.*(...)`: one run-scoped store operation as a leaf yield.
    /// Section VMs and the live H1 VM run the store shims; the agent
    /// driver carries an unreachable internal-invariant guard for the arm
    /// its exhaustive match forces (an agent VM's store table keeps the
    /// direct closures). Every operation takes this path uniformly -
    /// memory- and host-backed alike, with no inline fast path - so
    /// interleaving behavior never depends on the backend.
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
/// The read bounds stay `i64` exactly as the legacy callback's signature
/// had them: a negative bound converts to 0 at execution, which the
/// facade's range validation rejects with the same error a zero bound
/// earns.
#[derive(Debug)]
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

/// One normalized tool call an assistant message carries: the
/// provider-neutral `{id, name, arguments}` record every later component
/// consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// The call identifier tool results correlate against.
    pub id: String,
    /// The wire name of the tool the model asked for.
    pub name: String,
    /// The call arguments; always an object, normalized to `{}` when the
    /// record carried none.
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
    /// The normalized tool calls the record carries; empty unless an
    /// assistant turn requested tools.
    pub tool_calls: Vec<ToolCallRecord>,
    /// The call ID a tool result answers; required on `tool` records.
    pub tool_call_id: Option<String>,
}
