//! The coroutine protocol: validated request and answer types for the
//! yield/resume boundary between section Lua and the scheduler driver.
//!
//! A suspending host call (`models.infer`, `handle:infer`, `execute`,
//! `fanout`, `tool_call`, the agent-only `models.chat`) is a Lua-side shim
//! that yields a request table; the driver validates the yield into a
//! [`Request`], dispatches it, and resumes the coroutine with the
//! `(ok, result)` envelope rendered from an [`Answer`]. The two enums are
//! the audit surface: what a script can cause the host to do is one short
//! read, and each variant's fields are the compiler-checked per-message
//! contract.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use promptforge_core_support::events::{CallMetrics, ToolCallEvent};
use promptforge_model_client::model::ModelBinding;

use crate::{
    Error, LuaFanoutResult, LuaModelHandle, Result, ToolOutputKind, pack_sequence,
    resolve_section_target,
};

/// The fixed failure for a yield that is not a well-formed request table.
///
/// The coroutine global is stripped from author reach, so the only yields in
/// a well-formed run are shim yields, which are well-formed by construction;
/// anything else is a hand-rolled or corrupted yield and fails the block as a
/// loud authoring error rather than confusing the driver.
const DIRECT_YIELD: &str = "scripts may not yield directly";

/// The fixed direct-yield failure.
fn direct_yield_error() -> Error {
    Error::Lua(DIRECT_YIELD.to_owned())
}

/// Fails the block with the fixed direct-yield message.
fn direct_yield<T>() -> Result<T> {
    Err(direct_yield_error())
}

/// Reads one field off the request table.
///
/// Reads are raw: the table comes from script space, so a metatable must not
/// intercept or forge a field.
fn raw_field(table: &mlua::Table, name: &str) -> Result<Value> {
    table.raw_get::<Value>(name).or_else(|_| direct_yield())
}

/// Reads a required plain-table field as its JSON snapshot.
fn json_field(lua: &Lua, table: &mlua::Table, name: &str) -> Result<serde_json::Value> {
    match raw_field(table, name)? {
        value @ Value::Table(_) => lua.from_value(value).or_else(|_| direct_yield()),
        _ => direct_yield(),
    }
}

/// How reading one request field failed.
enum FieldFailure {
    /// A shim-internal field was absent or unreadable: the shims set those
    /// fields by construction, so the yield is malformed.
    Malformed,
    /// An author-supplied argument had the wrong shape: the call's error,
    /// resumed as the answer so the shim raises it at the call site - an
    /// author `pcall` catches it, exactly as the legacy callback's argument
    /// error surfaced.
    Call(Error),
}

/// Reads one author-supplied required string argument. Every wrong shape,
/// absent included, is the call's error: the legacy callback's argument
/// conversion failed at the call site too.
fn call_string(table: &mlua::Table, name: &str) -> std::result::Result<String, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::String(value)) => value.to_str().map(|value| value.to_owned()).map_err(|_| {
            FieldFailure::Call(Error::Lua(format!("{name} must be a valid UTF-8 string")))
        }),
        Ok(other) => Err(FieldFailure::Call(Error::Lua(format!(
            "{name} must be a string, got {}",
            other.type_name()
        )))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads one author-supplied optional string argument: absent or nil is
/// `None`, any other wrong shape is the call's error.
fn call_optional_string(
    table: &mlua::Table,
    name: &str,
) -> std::result::Result<Option<String>, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::String(value)) => {
            value
                .to_str()
                .map(|value| Some(value.to_owned()))
                .map_err(|_| {
                    FieldFailure::Call(Error::Lua(format!("{name} must be a valid UTF-8 string")))
                })
        }
        Ok(other) => Err(FieldFailure::Call(Error::Lua(format!(
            "{name} must be a string, got {}",
            other.type_name()
        )))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads the shim-produced `var` snapshot; a failure is a malformed yield,
/// since the snapshot helper produces a plain JSON-representable table by
/// construction.
fn shim_var(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<serde_json::Value, FieldFailure> {
    json_field(lua, table, "var").map_err(|_| FieldFailure::Malformed)
}

/// A validated suspending host call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: the
/// fanout collection converts through the existing member-wise rules and the
/// handle userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
/// lifetime-bound enters the enum.
#[derive(Debug)]
pub enum Request {
    /// `models.infer` (`binding: None`: resolve the section's current model)
    /// or `handle:infer` (`binding: Some`: the handle's frozen binding).
    Infer {
        /// The author-supplied prompt text.
        prompt: String,
        /// The handle's frozen binding for `handle:infer`, else `None`.
        binding: Option<ModelBinding>,
    },
    /// `execute(target, input?)`: run a contained chain over the target's
    /// slice.
    Execute {
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
    /// `tool_call(alias, args)`: suspending dispatch of a bound tool
    /// through the shared dispatch function.
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
        /// The validated message array. Each entry carries a known role
        /// (`system`, `user`, `assistant`, `tool`) and a `content` that is
        /// a string or a non-empty content-parts array (known part types:
        /// `text`, `image_url`); tool entries carry a string
        /// `tool_call_id`. Validation lives here, in the protocol parse,
        /// once - the driver converts without re-checking.
        messages: serde_json::Value,
        /// `opts.model`: the catalog model to use for this round, or
        /// `None` for the program's current `models.use` selection.
        model: Option<String>,
        /// `opts.tools`: the tool aliases to advertise for exactly this
        /// round. Defaults to none; the driver never adds to it.
        tools: Vec<String>,
    },
    /// Reserved. Never dispatched: receiving one is a typed protocol error.
    // The fields are read only by this module's own tests; production parses
    // them for strict validation and never reads them until the variant
    // gains a dispatch.
    #[allow(dead_code)]
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
    /// Validates a yielded value at the resume boundary.
    ///
    /// Every field is checked before use: the table comes from script space.
    /// A yield that is not a well-formed request table (not a table, no
    /// `op`, an unknown `op`, a shim-internal field of the wrong shape) is
    /// [`YieldParse::Malformed`] and fails the block with "scripts may not
    /// yield directly". A well-formed shim call whose author-supplied
    /// argument fails validation is [`YieldParse::Call`]: the error rides
    /// back as the call's answer so the shim raises it at the call site,
    /// keeping the legacy callback's errors catchable by an author `pcall`.
    /// Two boundary conversions keep their own byte-identical errors: an
    /// `execute` target that is not a string fails as
    /// `resolve_section_target` fails, and a fanout collection fails as
    /// `collection_to_items` fails.
    pub fn from_yield(lua: &Lua, yielded: &Value) -> YieldParse {
        let Value::Table(table) = yielded else {
            return YieldParse::Malformed(direct_yield_error());
        };
        let op = match raw_field(table, "op") {
            Ok(Value::String(op)) => match op.to_str() {
                Ok(op) => op.to_owned(),
                Err(_) => return YieldParse::Malformed(direct_yield_error()),
            },
            _ => return YieldParse::Malformed(direct_yield_error()),
        };
        match op.as_str() {
            "infer" => classify(parse_infer(table), |error| Answer::Infer(Err(error))),
            "execute" => classify(parse_execute(lua, table), |error| {
                Answer::Execute(Err(error))
            }),
            "fanout" => classify(parse_fanout(lua, table), |error| Answer::Fanout(Err(error))),
            "tool_call" => classify(parse_tool_call(lua, table), |error| {
                Answer::ToolCallResult(Err(error))
            }),
            "chat" => classify(parse_chat(lua, table), |error| Answer::Chat(Err(error))),
            "mcp" => match parse_mcp(lua, table) {
                Ok(request) => YieldParse::Request(request),
                Err(_) => YieldParse::Malformed(direct_yield_error()),
            },
            _ => YieldParse::Malformed(direct_yield_error()),
        }
    }

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

/// Maps one per-op parse to the boundary outcome: a validated request, an
/// author-argument failure as the call's answer, or a malformed yield.
fn classify(
    parsed: std::result::Result<Request, FieldFailure>,
    answer: impl FnOnce(Error) -> Answer<Error>,
) -> YieldParse {
    match parsed {
        Ok(request) => YieldParse::Request(request),
        Err(FieldFailure::Call(error)) => YieldParse::Call(answer(error)),
        Err(FieldFailure::Malformed) => YieldParse::Malformed(direct_yield_error()),
    }
}

/// Parses an `infer` request: the author-supplied `prompt`, and the
/// shim-produced `handle` userdata whose frozen [`ModelBinding`] is cloned
/// out of its borrow while the VM handle is live.
fn parse_infer(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let prompt = call_string(table, "prompt")?;
    let binding = match table.raw_get::<Value>("handle") {
        Ok(Value::Nil) => None,
        Ok(Value::UserData(userdata)) => match userdata.borrow::<LuaModelHandle>() {
            Ok(handle) => Some(handle.binding().clone()),
            Err(_) => return Err(FieldFailure::Malformed),
        },
        _ => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Infer { prompt, binding })
}

/// Parses an `execute` request: the author-supplied `target` (validated
/// with the `resolve_section_target` rule, keeping its byte-identical
/// error) and `input`, plus the shim-produced `var` snapshot.
fn parse_execute(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let target = match table.raw_get::<Value>("target") {
        Ok(value) => {
            resolve_section_target(value).map_err(|error| FieldFailure::Call(Error::lua(error)))?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let input = call_optional_string(table, "input")?;
    let var = shim_var(lua, table)?;
    Ok(Request::Execute { target, input, var })
}

/// Parses a `fanout` request: the author-supplied `worker` heading and
/// `collection` (converted member-wise while the VM handle is live, keeping
/// the conversion's byte-identical errors), plus the shim-produced `var`
/// snapshot.
fn parse_fanout(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let worker = call_string(table, "worker")?;
    let items = match table.raw_get::<Value>("collection") {
        Ok(collection) => {
            crate::collection::collection_to_items(lua, &collection).map_err(FieldFailure::Call)?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let var = shim_var(lua, table)?;
    Ok(Request::Fanout { worker, items, var })
}

/// Parses a `tool_call` request: the author-supplied `alias` and `args`.
///
/// An absent or nil `args` parses as the empty object (the empty-argument
/// call every tool accepts). A non-table or JSON-unrepresentable `args` is
/// the call's error, framed exactly as the other author-argument failures,
/// so an author `pcall` catches it at the call site.
fn parse_tool_call(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let alias = call_string(table, "alias")?;
    let args = match table.raw_get::<Value>("args") {
        Ok(Value::Nil) => serde_json::Value::Object(serde_json::Map::new()),
        Ok(Value::Table(_)) => json_field(lua, table, "args").map_err(|_| {
            FieldFailure::Call(Error::Lua(
                "args must be a JSON-representable table".to_owned(),
            ))
        })?,
        Ok(other) => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "args must be a table, got {}",
                other.type_name()
            ))));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::ToolCall { alias, args })
}

/// Parses a reserved `mcp` request. No call surface produces one, so every
/// field is shim-internal by construction.
fn parse_mcp(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let server = call_string(table, "server")?;
    let tool = call_string(table, "tool")?;
    let args = json_field(lua, table, "args").map_err(|_| FieldFailure::Malformed)?;
    Ok(Request::Mcp { server, tool, args })
}

/// The message roles the chat protocol accepts.
const CHAT_ROLES: [&str; 4] = ["system", "user", "assistant", "tool"];

/// The content-part types the chat protocol accepts (the Multimodal
/// contract: text parts and data-URI image parts).
const CHAT_PART_TYPES: [&str; 2] = ["text", "image_url"];

/// Frames one chat author-argument failure as the call's error.
fn chat_error(message: impl Into<String>) -> FieldFailure {
    FieldFailure::Call(Error::Lua(message.into()))
}

/// Parses a `chat` request: the author-supplied `messages` list and the
/// optional `opts` table carrying `model` and `tools`.
///
/// The whole messages/opts validation lives here, once - the driver
/// converts the validated array without re-checking. Every author-argument
/// failure is the call's error, raised at the `models.chat` call site so a
/// program `pcall` catches it.
fn parse_chat(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let messages = match table.raw_get::<Value>("messages") {
        Ok(value @ Value::Table(_)) => lua
            .from_value::<serde_json::Value>(value)
            .map_err(|_| chat_error("messages must be a JSON-representable table"))?,
        Ok(other) => {
            return Err(chat_error(format!(
                "messages must be a table of message tables, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    validate_messages(&messages)?;
    let (model, tools) = parse_chat_opts(table)?;
    Ok(Request::Chat {
        messages,
        model,
        tools,
    })
}

/// Validates the converted message array once, at the protocol boundary:
/// known roles; `content` a string or a non-empty content-parts array with
/// known part types; tool entries carry a string `tool_call_id`; a present
/// `tool_calls` is an array. The empty list is rejected, and every error
/// names the offending 1-based index (the list is Lua-authored). The
/// validation is deliberately shallow: `content` and `tool_calls`
/// internals pass to the wire unread, while entry fields beyond the four
/// the wire message carries (`role`, `content`, `tool_call_id`,
/// `tool_calls`) are accepted here and dropped at the driver's wire
/// conversion.
fn validate_messages(messages: &serde_json::Value) -> std::result::Result<(), FieldFailure> {
    let entries = match messages {
        serde_json::Value::Array(entries) => entries,
        // An empty Lua table converts ambiguously (array or object); both
        // empty shapes are the same authoring error, named the same way.
        serde_json::Value::Object(map) if map.is_empty() => {
            return Err(chat_error("messages must not be empty"));
        }
        _ => return Err(chat_error("messages must be an array of message tables")),
    };
    if entries.is_empty() {
        return Err(chat_error("messages must not be empty"));
    }
    for (position, entry) in entries.iter().enumerate() {
        // The list is Lua-authored, so errors name Lua's 1-based index.
        let index = position + 1;
        let serde_json::Value::Object(entry) = entry else {
            return Err(chat_error(format!(
                "messages[{index}] must be a message table"
            )));
        };
        let role = match entry.get("role") {
            Some(serde_json::Value::String(role)) => role.as_str(),
            _ => {
                return Err(chat_error(format!(
                    "messages[{index}] role must be a string, one of: {}",
                    CHAT_ROLES.join(", ")
                )));
            }
        };
        if !CHAT_ROLES.contains(&role) {
            return Err(chat_error(format!(
                "messages[{index}] role {role:?} is unknown; known roles: {}",
                CHAT_ROLES.join(", ")
            )));
        }
        match entry.get("content") {
            Some(serde_json::Value::String(_)) => {}
            Some(serde_json::Value::Array(parts)) if !parts.is_empty() => {
                validate_content_parts(index, parts)?;
            }
            _ => {
                return Err(chat_error(format!(
                    "messages[{index}] content must be a string or a non-empty \
                     array of content parts"
                )));
            }
        }
        if role == "tool"
            && !matches!(
                entry.get("tool_call_id"),
                Some(serde_json::Value::String(_))
            )
        {
            return Err(chat_error(format!(
                "messages[{index}] is a tool message and must carry a string tool_call_id"
            )));
        }
        if let Some(calls) = entry.get("tool_calls")
            && !calls.is_array()
        {
            return Err(chat_error(format!(
                "messages[{index}] tool_calls must be an array"
            )));
        }
    }
    Ok(())
}

/// Shallow-validates one message's content-parts array: each part is a
/// table whose `type` names a known part kind. Part internals pass through
/// to the wire unread.
fn validate_content_parts(
    index: usize,
    parts: &[serde_json::Value],
) -> std::result::Result<(), FieldFailure> {
    for (part_position, part) in parts.iter().enumerate() {
        let part_index = part_position + 1;
        let serde_json::Value::Object(part) = part else {
            return Err(chat_error(format!(
                "messages[{index}] content part {part_index} must be a table \
                 with a string type field"
            )));
        };
        match part.get("type") {
            Some(serde_json::Value::String(kind)) if CHAT_PART_TYPES.contains(&kind.as_str()) => {}
            Some(serde_json::Value::String(kind)) => {
                return Err(chat_error(format!(
                    "messages[{index}] content part {part_index} has unknown type \
                     {kind:?}; known types: {}",
                    CHAT_PART_TYPES.join(", ")
                )));
            }
            _ => {
                return Err(chat_error(format!(
                    "messages[{index}] content part {part_index} must be a table \
                     with a string type field"
                )));
            }
        }
    }
    Ok(())
}

/// Parses the optional `opts` table: `model` (an optional catalog model
/// name) and `tools` (the aliases to advertise this round; default none).
fn parse_chat_opts(
    table: &mlua::Table,
) -> std::result::Result<(Option<String>, Vec<String>), FieldFailure> {
    let opts = match table.raw_get::<Value>("opts") {
        Ok(Value::Nil) => return Ok((None, Vec::new())),
        Ok(Value::Table(opts)) => opts,
        Ok(other) => {
            return Err(chat_error(format!(
                "opts must be a table, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let model = match opts.raw_get::<Value>("model") {
        Ok(Value::Nil) => None,
        Ok(Value::String(name)) => Some(
            name.to_str()
                .map_err(|_| chat_error("opts.model must be a valid UTF-8 string"))?
                .to_owned(),
        ),
        Ok(other) => {
            return Err(chat_error(format!(
                "opts.model must be a string, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let tools = match opts.raw_get::<Value>("tools") {
        Ok(Value::Nil) => Vec::new(),
        Ok(Value::Table(aliases)) => {
            let mut tools = Vec::new();
            for (position, alias) in aliases.sequence_values::<Value>().enumerate() {
                let alias_index = position + 1;
                match alias {
                    Ok(Value::String(alias)) => tools.push(
                        alias
                            .to_str()
                            .map_err(|_| {
                                chat_error(format!(
                                    "opts.tools[{alias_index}] must be a valid UTF-8 string"
                                ))
                            })?
                            .to_owned(),
                    ),
                    Ok(other) => {
                        return Err(chat_error(format!(
                            "opts.tools[{alias_index}] must be a string tool alias, got {}",
                            other.type_name()
                        )));
                    }
                    Err(_) => return Err(FieldFailure::Malformed),
                }
            }
            tools
        }
        Ok(other) => {
            return Err(chat_error(format!(
                "opts.tools must be an array of tool alias strings, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok((model, tools))
}

/// How one yielded value parsed at the resume boundary.
#[derive(Debug)]
pub enum YieldParse {
    /// A well-formed request, ready to dispatch.
    Request(Request),
    /// A well-formed shim call whose author-supplied argument failed
    /// validation: the call's answer, resumed into the caller so the shim
    /// raises the error at the call site, exactly as the legacy callback's
    /// argument error surfaced.
    Call(Answer<Error>),
    /// Not a well-formed request table: a hand-rolled or corrupted yield,
    /// failing the block with the fixed direct-yield message.
    Malformed(Error),
}

/// One dispatched `tool_call`'s successful output, classified by the
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

/// Renders one [`ChatResult`] as the plain Lua result table.
///
/// Absent optional fields are never set, so they resume as nil and
/// `result.tool_calls` presence-branching works; mapping them through the
/// serde boundary would resume mlua's non-nil null sentinel instead. Each
/// call's `arguments` and the `metrics` sections cross the serde boundary
/// as tables (the metrics types skip absent sections in serialization, so
/// no null enters them).
fn chat_result_table(lua: &Lua, result: ChatResult) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    if let Some(reply) = result.reply {
        table.raw_set("reply", reply)?;
    }
    if let Some(calls) = result.tool_calls {
        let sequence = lua.create_table_with_capacity(calls.len(), 0)?;
        for (position, call) in calls.into_iter().enumerate() {
            let entry = lua.create_table()?;
            entry.raw_set("id", call.id)?;
            entry.raw_set("name", call.name)?;
            entry.raw_set("arguments", lua.to_value(&call.arguments)?)?;
            sequence.raw_set(position + 1, entry)?;
        }
        table.raw_set("tool_calls", sequence)?;
    }
    if let Some(finish_reason) = result.finish_reason {
        table.raw_set("finish_reason", finish_reason)?;
    }
    table.raw_set("model", result.model)?;
    if let Some(metrics) = result.metrics {
        table.raw_set("metrics", lua.to_value(&metrics)?)?;
    }
    Ok(table)
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
/// until the envelope is rendered, so an `Execute` or `Fanout` failure
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
    /// The contained chain's final text for an `execute` request.
    Execute(std::result::Result<String, E>),
    /// The ordered arm results for a `fanout` request, in collection order.
    Fanout(std::result::Result<Vec<LuaFanoutResult>, E>),
    /// The classified output for a `chat` request. Boxed so the metrics-heavy
    /// [`ChatResult`] does not size every answer the non-chat paths move.
    Chat(std::result::Result<Box<ChatResult>, E>),
    /// The classified output for a `tool_call` request.
    ToolCallResult(std::result::Result<ToolCallOutcome, E>),
}

impl<E> Answer<E> {
    /// Maps the carried error type, leaving every success value untouched.
    pub fn map_error<F>(self, map: impl FnOnce(E) -> F) -> Answer<F> {
        match self {
            Answer::Infer(result) => Answer::Infer(result.map_err(map)),
            Answer::Execute(result) => Answer::Execute(result.map_err(map)),
            Answer::Fanout(result) => Answer::Fanout(result.map_err(map)),
            Answer::ToolCallResult(result) => Answer::ToolCallResult(result.map_err(map)),
            Answer::Chat(result) => Answer::Chat(result.map_err(map)),
        }
    }
}

impl<E: std::fmt::Display> Answer<E> {
    /// Renders the `(ok, result)` resume values for the shim.
    ///
    /// On success the envelope is `(true, text)` or, for a fanout, `(true,
    /// sequence)` with the packed 1-based result table built on the chain's
    /// VM. On failure it is `(false, message)`, where `message` is the
    /// error's display string - the shim raises it with `error(result, 0)`,
    /// so the author sees exactly the host's message - and the typed
    /// [`Error`] is returned alongside for the driver to retain.
    ///
    /// # Errors
    /// Returns an `mlua` error if a Lua string, userdata, or table cannot be
    /// created on `lua`.
    pub fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<E>)> {
        match self {
            Answer::Infer(Ok(text))
            | Answer::Execute(Ok(text))
            | Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(text))) => {
                let text = lua.create_string(&text)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::String(text)]),
                    None,
                ))
            }
            Answer::ToolCallResult(Ok(ToolCallOutcome::Structured(json))) => {
                // The one serde-boundary conversion: the parsed JSON output
                // becomes the resumed Lua value, so the shim hands the
                // script a table with no codec in author reach.
                let value = lua.to_value(&json)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), value]),
                    None,
                ))
            }
            Answer::Fanout(Ok(results)) => {
                let mut handles = Vec::with_capacity(results.len());
                for result in results {
                    handles.push(lua.create_userdata(result)?);
                }
                let sequence = pack_sequence(lua, handles)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::Table(sequence)]),
                    None,
                ))
            }
            Answer::Chat(Ok(result)) => {
                let table = chat_result_table(lua, *result)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::Table(table)]),
                    None,
                ))
            }
            Answer::Infer(Err(error))
            | Answer::Execute(Err(error))
            | Answer::Fanout(Err(error))
            | Answer::ToolCallResult(Err(error))
            | Answer::Chat(Err(error)) => {
                let message = lua.create_string(error.to_string())?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(false), Value::String(message)]),
                    Some(error),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use mlua::{AnyUserData, Function};
    use serde_json::json;

    use super::*;
    use promptforge_model_client::model::{ModelId, ModelInvocation};

    fn test_binding() -> ModelBinding {
        ModelBinding::new(
            "fast",
            "a fast model",
            ModelId::from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )
    }

    fn handle_userdata(lua: &Lua) -> AnyUserData {
        lua.create_userdata(LuaModelHandle::from_binding(&test_binding()))
            .expect("userdata creation cannot fail on a fresh VM")
    }

    fn request_table(lua: &Lua, op: &str) -> mlua::Table {
        let table = lua.create_table().expect("table creation cannot fail");
        table
            .raw_set("op", op)
            .expect("raw_set on a fresh table cannot fail");
        table
    }

    fn set_var_snapshot(lua: &Lua, table: &mlua::Table) {
        let var = lua.create_table().expect("table creation cannot fail");
        var.raw_set("k", 1)
            .expect("raw_set on a fresh table cannot fail");
        table
            .raw_set("var", var)
            .expect("raw_set on a fresh table cannot fail");
    }

    fn assert_direct_yield(parse: YieldParse) {
        match parse {
            YieldParse::Malformed(Error::Lua(message)) => {
                assert_eq!(message, "scripts may not yield directly");
            }
            other => panic!("expected the direct-yield Lua error, got {other:?}"),
        }
    }

    fn expect_request(parse: YieldParse) -> Request {
        match parse {
            YieldParse::Request(request) => request,
            other => panic!("expected a well-formed request, got {other:?}"),
        }
    }

    fn echo_through_lua(lua: &Lua, envelope: MultiValue) -> (bool, Value) {
        let echo: Function = lua
            .create_function(|_, (ok, result): (bool, Value)| Ok((ok, result)))
            .expect("echo function creation cannot fail");
        echo.call::<(bool, Value)>(envelope)
            .expect("the envelope round-trips through Lua")
    }

    #[test]
    fn infer_without_a_handle_parses() {
        let lua = Lua::new();
        let table = request_table(&lua, "infer");
        table.raw_set("prompt", "summarize this").expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Infer { prompt, binding } => {
                assert_eq!(prompt, "summarize this");
                assert_eq!(binding, None);
            }
            other => panic!("expected an infer request, got {other:?}"),
        }
    }

    #[test]
    fn infer_with_a_handle_clones_its_frozen_binding() {
        let lua = Lua::new();
        let table = request_table(&lua, "infer");
        table.raw_set("prompt", "hi").expect("raw_set");
        table
            .raw_set("handle", handle_userdata(&lua))
            .expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Infer {
                binding: Some(binding),
                ..
            } => {
                assert_eq!(binding.alias(), "fast");
                assert_eq!(binding.id().name(), "test-model");
            }
            other => panic!("expected an infer request with a binding, got {other:?}"),
        }
    }

    #[test]
    fn execute_parses_target_input_and_var_snapshot() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        table.raw_set("input", "override").expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Execute { target, input, var } => {
                assert_eq!(target, "## Child");
                assert_eq!(input.as_deref(), Some("override"));
                assert_eq!(var, json!({ "k": 1 }));
            }
            other => panic!("expected an execute request, got {other:?}"),
        }
    }

    #[test]
    fn execute_without_input_yields_none() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Execute { input, .. } => assert_eq!(input, None),
            other => panic!("expected an execute request, got {other:?}"),
        }
    }

    #[test]
    fn fanout_parses_and_converts_the_collection_member_wise() {
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", "### Worker").expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        collection.raw_set(1, "a").expect("raw_set");
        collection.raw_set(2, 2).expect("raw_set");
        collection.raw_set("key", true).expect("raw_set");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Fanout { worker, items, var } => {
                assert_eq!(worker, "### Worker");
                assert_eq!(
                    items,
                    vec![json!("a"), json!(2), json!({ "key": "key", "value": true })]
                );
                assert_eq!(var, json!({ "k": 1 }));
            }
            other => panic!("expected a fanout request, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_parses_alias_and_args() {
        let lua = Lua::new();
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", "echo").expect("raw_set");
        let args = lua.create_table().expect("table creation cannot fail");
        args.raw_set("value", "hi").expect("raw_set");
        table.raw_set("args", args).expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::ToolCall { alias, args } => {
                assert_eq!(alias, "echo");
                assert_eq!(args, json!({ "value": "hi" }));
            }
            other => panic!("expected a tool_call request, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_without_args_parses_the_empty_object() {
        let lua = Lua::new();
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", "echo").expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::ToolCall { args, .. } => assert_eq!(args, json!({})),
            other => panic!("expected a tool_call request, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_call_with_a_non_string_alias_is_the_calls_error() {
        // The author-facing argument error rides back as the call's answer,
        // framed byte-identically with the other author-argument failures.
        let lua = Lua::new();
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", 42).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
                assert_eq!(message, "alias must be a string, got integer");
            }
            other => panic!("expected the alias call error, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_call_with_a_non_table_args_is_the_calls_error() {
        let lua = Lua::new();
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", "echo").expect("raw_set");
        table.raw_set("args", 42).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
                assert_eq!(message, "args must be a table, got integer");
            }
            other => panic!("expected the args call error, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_call_with_an_unrepresentable_args_table_is_the_calls_error() {
        let lua = Lua::new();
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", "echo").expect("raw_set");
        let args = lua.create_table().expect("table creation cannot fail");
        let member = lua
            .create_function(|_, ()| Ok(()))
            .expect("function creation cannot fail");
        args.raw_set("f", member).expect("raw_set");
        table.raw_set("args", args).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
                assert_eq!(message, "args must be a JSON-representable table");
            }
            other => panic!("expected the args call error, got {other:?}"),
        }
    }

    #[test]
    fn an_ok_plain_tool_call_answer_round_trips_as_a_string() {
        let lua = Lua::new();
        let (envelope, retained) =
            Answer::<Error>::ToolCallResult(Ok(ToolCallOutcome::Plain("echoed: hi".to_owned())))
                .into_envelope(&lua)
                .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(ok);
        let Value::String(text) = result else {
            panic!("expected a string result, got {result:?}");
        };
        assert_eq!(text.to_str().expect("the text is UTF-8"), "echoed: hi");
    }

    #[test]
    fn an_ok_structured_tool_call_answer_round_trips_as_a_table() {
        let lua = Lua::new();
        let outcome = ToolCallOutcome::Structured(json!({ "text": "typed", "images": [] }));
        let (envelope, retained) = Answer::<Error>::ToolCallResult(Ok(outcome))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, text, images_len): (bool, String, i64) = lua
            .load("local ok, result = ...; return ok, result.text, #result.images")
            .call(envelope)
            .expect("the table reads back through Lua");
        assert!(ok);
        assert_eq!(text, "typed");
        assert_eq!(images_len, 0);
    }

    #[test]
    fn an_err_tool_call_answer_round_trips_and_retains_the_typed_error() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::ToolCallResult(Err(Error::Interrupted))
            .into_envelope(&lua)
            .expect("the envelope renders");
        match retained {
            Some(Error::Interrupted) => {}
            other => panic!("expected the retained Interrupted error, got {other:?}"),
        }
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(!ok);
        let Value::String(message) = result else {
            panic!("expected a string message, got {result:?}");
        };
        assert_eq!(
            message.to_str().expect("the message is UTF-8"),
            "interrupted by Ctrl-C"
        );
    }

    #[test]
    fn from_dispatch_classifies_by_the_declared_output_kind() {
        use crate::ToolOutputKind;

        // Plain output passes through untouched.
        match ToolCallOutcome::from_dispatch(ToolOutputKind::Plain, "echo", "raw".to_owned()) {
            Ok(ToolCallOutcome::Plain(text)) => assert_eq!(text, "raw"),
            other => panic!("expected the plain passthrough, got {other:?}"),
        }
        // Structured output parses as JSON.
        match ToolCallOutcome::from_dispatch(
            ToolOutputKind::Structured,
            "form",
            "{\"text\":\"hi\"}".to_owned(),
        ) {
            Ok(ToolCallOutcome::Structured(json)) => assert_eq!(json, json!({ "text": "hi" })),
            other => panic!("expected the structured parse, got {other:?}"),
        }
        // Invalid JSON from a structured binding is the tool's error.
        match ToolCallOutcome::from_dispatch(
            ToolOutputKind::Structured,
            "form",
            "not json".to_owned(),
        ) {
            Err(Error::Tool { message, source }) => {
                assert_eq!(message, "structured tool \"form\" returned invalid JSON");
                assert!(
                    source.downcast_ref::<serde_json::Error>().is_some(),
                    "the parse failure must survive as the cause"
                );
            }
            other => panic!("expected the typed tool error, got {other:?}"),
        }
    }

    /// Evaluates a Lua table constructor, so chat tests build author-shaped
    /// message and opts tables from the exact source an author would write.
    fn lua_table(lua: &Lua, source: &str) -> mlua::Table {
        lua.load(source)
            .eval()
            .expect("test table source evaluates")
    }

    fn chat_request(lua: &Lua, messages: &str, opts: Option<&str>) -> mlua::Table {
        let table = request_table(lua, "chat");
        table
            .raw_set("messages", lua_table(lua, messages))
            .expect("raw_set");
        if let Some(opts) = opts {
            table
                .raw_set("opts", lua_table(lua, opts))
                .expect("raw_set");
        }
        table
    }

    fn expect_chat_call_error(parse: YieldParse, expected: &str) {
        match parse {
            YieldParse::Call(Answer::Chat(Err(Error::Lua(message)))) => {
                assert_eq!(message, expected);
            }
            other => panic!("expected the chat call error {expected:?}, got {other:?}"),
        }
    }

    #[test]
    fn chat_parses_messages_model_and_tools() {
        let lua = Lua::new();
        let table = chat_request(
            &lua,
            r#"{
                { role = "system", content = "be terse" },
                { role = "user", content = {
                    { type = "text", text = "look" },
                    { type = "image_url", image_url = { url = "data:image/png;base64,AA" } },
                } },
                { role = "assistant", content = "", tool_calls = {
                    { id = "call_1" },
                } },
                { role = "tool", content = "result", tool_call_id = "call_1" },
            }"#,
            Some(r#"{ model = "fast", tools = { "echo", "search" } }"#),
        );
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Chat {
                messages,
                model,
                tools,
            } => {
                assert_eq!(model.as_deref(), Some("fast"));
                assert_eq!(tools, vec!["echo".to_owned(), "search".to_owned()]);
                let entries = messages.as_array().expect("messages parse as an array");
                assert_eq!(entries.len(), 4);
                assert_eq!(entries[0]["role"], json!("system"));
                assert_eq!(
                    entries[1]["content"][0],
                    json!({ "type": "text", "text": "look" }),
                    "content parts must survive the conversion verbatim"
                );
                assert_eq!(entries[3]["tool_call_id"], json!("call_1"));
            }
            other => panic!("expected a chat request, got {other:?}"),
        }
    }

    #[test]
    fn chat_without_opts_defaults_to_no_model_and_no_tools() {
        let lua = Lua::new();
        let table = chat_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Chat { model, tools, .. } => {
                assert_eq!(model, None);
                assert_eq!(
                    tools,
                    Vec::<String>::new(),
                    "the advertised set defaults to none"
                );
            }
            other => panic!("expected a chat request, got {other:?}"),
        }
    }

    #[test]
    fn chat_message_validation_names_the_offending_index() {
        let lua = Lua::new();
        let cases: [(&str, &str); 8] = [
            ("{}", "messages must not be empty"),
            (
                r#"{ "not a table" }"#,
                "messages[1] must be a message table",
            ),
            (
                r#"{ { role = "user", content = "ok" }, { role = "wizard", content = "x" } }"#,
                "messages[2] role \"wizard\" is unknown; known roles: system, user, assistant, tool",
            ),
            (
                r#"{ { content = "no role" } }"#,
                "messages[1] role must be a string, one of: system, user, assistant, tool",
            ),
            (
                r#"{ { role = "user" } }"#,
                "messages[1] content must be a string or a non-empty array of content parts",
            ),
            (
                r#"{ { role = "user", content = { "bare string part" } } }"#,
                "messages[1] content part 1 must be a table with a string type field",
            ),
            (
                r#"{ { role = "user", content = { { type = "text", text = "ok" }, { type = "video" } } } }"#,
                "messages[1] content part 2 has unknown type \"video\"; known types: text, image_url",
            ),
            (
                r#"{ { role = "user", content = "ok" }, { role = "tool", content = "r" } }"#,
                "messages[2] is a tool message and must carry a string tool_call_id",
            ),
        ];
        for (messages, expected) in cases {
            let table = chat_request(&lua, messages, None);
            expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
        }
        // A non-table messages argument, absent included, is the call's error.
        let missing = request_table(&lua, "chat");
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(missing)),
            "messages must be a table of message tables, got nil",
        );
        let numeric = request_table(&lua, "chat");
        numeric.raw_set("messages", 42).expect("raw_set");
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(numeric)),
            "messages must be a table of message tables, got integer",
        );
        // A present tool_calls of the wrong shape is rejected in place.
        let table = chat_request(
            &lua,
            r#"{ { role = "assistant", content = "", tool_calls = "raw" } }"#,
            None,
        );
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(table)),
            "messages[1] tool_calls must be an array",
        );
    }

    #[test]
    fn chat_opts_validation_is_the_calls_error() {
        let lua = Lua::new();
        let valid = r#"{ { role = "user", content = "hi" } }"#;
        let non_table = request_table(&lua, "chat");
        non_table
            .raw_set("messages", lua_table(&lua, valid))
            .expect("raw_set");
        non_table.raw_set("opts", "loud").expect("raw_set");
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(non_table)),
            "opts must be a table, got string",
        );
        let bad_model = chat_request(&lua, valid, Some("{ model = 42 }"));
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(bad_model)),
            "opts.model must be a string, got integer",
        );
        let bad_tools = chat_request(&lua, valid, Some(r#"{ tools = "echo" }"#));
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(bad_tools)),
            "opts.tools must be an array of tool alias strings, got string",
        );
        let bad_alias = chat_request(&lua, valid, Some(r#"{ tools = { "echo", 7 } }"#));
        expect_chat_call_error(
            Request::from_yield(&lua, &Value::Table(bad_alias)),
            "opts.tools[2] must be a string tool alias, got integer",
        );
    }

    #[test]
    fn an_ok_chat_reply_answer_resumes_as_a_table_with_nil_tool_calls() {
        use promptforge_core_support::events::{ClientTiming, Usage};

        let lua = Lua::new();
        let result = ChatResult {
            reply: Some("hello there".to_owned()),
            tool_calls: None,
            finish_reason: Some("stop".to_owned()),
            model: "fixture-model".to_owned(),
            metrics: Some(CallMetrics {
                usage: Some(Usage {
                    prompt_tokens: 7,
                    completion_tokens: 3,
                    total_tokens: 10,
                    cached_tokens: None,
                    reasoning_tokens: None,
                }),
                llama: None,
                vllm: None,
                client: Some(ClientTiming {
                    ttft_ms: Some(9.5),
                    mean_itl_ms: None,
                    e2e_ms: 41.5,
                }),
            }),
        };
        let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        // Presence-branching is the agent contract: absent fields must read
        // back as true Lua nil, never a serde null sentinel.
        let (ok, reply, tools_nil, finish, model, total, llama_nil, e2e): (
            bool,
            String,
            bool,
            String,
            String,
            i64,
            bool,
            f64,
        ) = lua
            .load(
                "local ok, r = ...; \
                 return ok, r.reply, r.tool_calls == nil, r.finish_reason, r.model, \
                 r.metrics.usage.total_tokens, r.metrics.llama == nil, r.metrics.client.e2e_ms",
            )
            .call(envelope)
            .expect("the result table reads back through Lua");
        assert!(ok);
        assert_eq!(reply, "hello there");
        assert!(
            tools_nil,
            "an absent tool_calls must be nil, not a null sentinel"
        );
        assert_eq!(finish, "stop");
        assert_eq!(model, "fixture-model");
        assert_eq!(total, 10);
        assert!(llama_nil, "an absent metrics section must be nil");
        assert!((e2e - 41.5).abs() < f64::EPSILON);
    }

    #[test]
    fn an_ok_chat_tool_calls_answer_resumes_with_presence_and_arguments() {
        let lua = Lua::new();
        let result = ChatResult {
            reply: None,
            tool_calls: Some(vec![
                ToolCallEvent {
                    id: "call_1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: json!({ "value": "hi" }),
                },
                ToolCallEvent {
                    id: "call_2".to_owned(),
                    name: "search".to_owned(),
                    arguments: json!({ "query": "rust" }),
                },
            ]),
            finish_reason: Some("tool_calls".to_owned()),
            model: "fixture-model".to_owned(),
            metrics: None,
        };
        let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, reply_nil, len, id, name, value, second, metrics_nil): (
            bool,
            bool,
            i64,
            String,
            String,
            String,
            String,
            bool,
        ) = lua
            .load(
                "local ok, r = ...; \
                 return ok, r.reply == nil, #r.tool_calls, r.tool_calls[1].id, \
                 r.tool_calls[1].name, r.tool_calls[1].arguments.value, \
                 r.tool_calls[2].arguments.query, r.metrics == nil",
            )
            .call(envelope)
            .expect("the result table reads back through Lua");
        assert!(ok);
        assert!(reply_nil, "a tool-calls round has no reply");
        assert_eq!(len, 2);
        assert_eq!(id, "call_1");
        assert_eq!(name, "echo");
        assert_eq!(value, "hi");
        assert_eq!(second, "rust");
        assert!(metrics_nil);
    }

    #[test]
    fn an_err_chat_answer_round_trips_and_retains_the_typed_error() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::Chat(Err(Error::Interrupted))
            .into_envelope(&lua)
            .expect("the envelope renders");
        match retained {
            Some(Error::Interrupted) => {}
            other => panic!("expected the retained Interrupted error, got {other:?}"),
        }
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(!ok);
        let Value::String(message) = result else {
            panic!("expected a string message, got {result:?}");
        };
        assert_eq!(
            message.to_str().expect("the message is UTF-8"),
            "interrupted by Ctrl-C"
        );
    }

    #[test]
    fn mcp_reserved_fields_parse() {
        let lua = Lua::new();
        let table = request_table(&lua, "mcp");
        table.raw_set("server", "srv").expect("raw_set");
        table.raw_set("tool", "tl").expect("raw_set");
        let args = lua.create_table().expect("table creation cannot fail");
        table.raw_set("args", args).expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Mcp { server, tool, args } => {
                assert_eq!(server, "srv");
                assert_eq!(tool, "tl");
                assert_eq!(args, json!({}));
            }
            other => panic!("expected an mcp request, got {other:?}"),
        }
    }

    #[test]
    fn a_received_mcp_request_is_a_typed_protocol_error() {
        match Request::mcp_reserved() {
            Error::Lua(message) => assert!(message.contains("mcp")),
            other => panic!("expected a typed Lua protocol error, got {other:?}"),
        }
    }

    #[test]
    fn a_non_table_yield_is_rejected() {
        let lua = Lua::new();
        assert_direct_yield(Request::from_yield(&lua, &Value::Integer(1)));
        let text = lua.create_string("infer").expect("string creation");
        assert_direct_yield(Request::from_yield(&lua, &Value::String(text)));
    }

    #[test]
    fn a_yield_without_an_op_is_rejected() {
        let lua = Lua::new();
        let table = lua.create_table().expect("table creation cannot fail");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_unknown_op_is_rejected() {
        let lua = Lua::new();
        let table = request_table(&lua, "teleport");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_infer_with_a_missing_or_non_string_prompt_is_the_calls_error() {
        // The author-facing argument error rides back as the call's answer,
        // so the shim raises it at the call site (pcall-able), exactly as
        // the legacy callback's conversion error surfaced.
        let lua = Lua::new();
        let missing = request_table(&lua, "infer");
        match Request::from_yield(&lua, &Value::Table(missing)) {
            YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
                assert_eq!(message, "prompt must be a string, got nil");
            }
            other => panic!("expected the prompt call error, got {other:?}"),
        }
        let typed_wrong = request_table(&lua, "infer");
        typed_wrong.raw_set("prompt", 42).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(typed_wrong)) {
            YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
                assert_eq!(message, "prompt must be a string, got integer");
            }
            other => panic!("expected the prompt call error, got {other:?}"),
        }
    }

    #[test]
    fn an_infer_with_a_wrong_handle_type_is_rejected() {
        let lua = Lua::new();
        let as_string = request_table(&lua, "infer");
        as_string.raw_set("prompt", "hi").expect("raw_set");
        as_string
            .raw_set("handle", "not a handle")
            .expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(as_string)));
        let as_other_userdata = request_table(&lua, "infer");
        as_other_userdata.raw_set("prompt", "hi").expect("raw_set");
        let wrong = lua
            .create_userdata(LuaFanoutResult::success(json!(1), "x"))
            .expect("userdata creation cannot fail on a fresh VM");
        as_other_userdata.raw_set("handle", wrong).expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(as_other_userdata)));
    }

    #[test]
    fn an_execute_with_a_non_string_target_keeps_the_resolve_error() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", 42).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Execute(Err(Error::LuaRuntime { message, .. }))) => {
                assert!(
                    message.contains("section target must be a string, got integer"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected the resolve_section_target call error, got {other:?}"),
        }
    }

    #[test]
    fn a_fanout_with_a_non_string_worker_is_the_calls_error() {
        // The author-facing argument error rides back as the call's answer,
        // so the shim raises it at the call site (pcall-able), exactly as
        // the legacy callback's conversion error surfaced.
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", 42).expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => {
                assert_eq!(message, "worker must be a string, got integer");
            }
            other => panic!("expected the worker call error, got {other:?}"),
        }
    }

    #[test]
    fn a_request_without_a_var_snapshot_is_rejected() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn fanout_collection_member_errors_stay_byte_identical() {
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", "### Worker").expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        let member = lua
            .create_function(|_, ()| Ok(()))
            .expect("function creation cannot fail");
        collection.raw_set(1, member).expect("raw_set");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => assert_eq!(
                message,
                "fanout collection member at index 1 is a function; members must be data"
            ),
            other => panic!("expected the collection member call error, got {other:?}"),
        }
    }

    #[test]
    fn metatable_spoofed_fields_are_not_read() {
        let lua = Lua::new();
        let table = lua.create_table().expect("table creation cannot fail");
        let index = lua.create_table().expect("table creation cannot fail");
        index.raw_set("op", "infer").expect("raw_set");
        index.raw_set("prompt", "hi").expect("raw_set");
        let metatable = lua.create_table().expect("table creation cannot fail");
        metatable.raw_set("__index", index).expect("raw_set");
        table
            .set_metatable(Some(metatable))
            .expect("set_metatable on a fresh table cannot fail");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_ok_infer_answer_round_trips_through_lua() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::<Error>::Infer(Ok("completion".to_owned()))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(ok);
        let Value::String(text) = result else {
            panic!("expected a string result, got {result:?}");
        };
        assert_eq!(text.to_str().expect("the text is UTF-8"), "completion");
    }

    #[test]
    fn an_ok_execute_answer_round_trips_through_lua() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::<Error>::Execute(Ok("chain text".to_owned()))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(ok);
        let Value::String(text) = result else {
            panic!("expected a string result, got {result:?}");
        };
        assert_eq!(text.to_str().expect("the text is UTF-8"), "chain text");
    }

    #[test]
    fn an_err_answer_round_trips_and_retains_the_typed_error() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::Execute(Err(Error::LuaQuota {
            resource: "instruction",
        }))
        .into_envelope(&lua)
        .expect("the envelope renders");
        match retained {
            Some(Error::LuaQuota {
                resource: "instruction",
            }) => {}
            other => panic!("expected the retained LuaQuota error, got {other:?}"),
        }
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(!ok);
        let Value::String(message) = result else {
            panic!("expected a string message, got {result:?}");
        };
        assert_eq!(
            message.to_str().expect("the message is UTF-8"),
            "lua instruction quota exceeded"
        );
    }

    #[test]
    fn an_ok_fanout_answer_round_trips_as_an_ordered_result_sequence() {
        let lua = Lua::new();
        let results = vec![
            LuaFanoutResult::success(json!("a"), "text-a"),
            LuaFanoutResult::exhausted_stub(json!("b"), "stub-b"),
        ];
        let (envelope, retained) = Answer::<Error>::Fanout(Ok(results))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, len, first_text, second_ok, second_exhausted, rendered): (
            bool,
            i64,
            String,
            bool,
            bool,
            String,
        ) = lua
            .load(
                "local ok, seq = ...; \
                 return ok, #seq, seq[1].text, seq[2].ok, seq[2].exhausted, tostring(seq[1])",
            )
            .call(envelope)
            .expect("the sequence reads back through Lua");
        assert!(ok);
        assert_eq!(len, 2);
        assert_eq!(first_text, "text-a");
        assert!(!second_ok);
        assert!(second_exhausted);
        assert_eq!(rendered, "text-a");
    }
}
