//! The rules of `models.loop`, run in Rust behind the loop shim.
//!
//! `models.loop` and a model handle's `loop` are one-line entries over a
//! Lua trampoline in `__impl_coro.lua` over `loop_begin`, which this
//! module builds, and the step closure each call returns, which owns that
//! call's machine. A Rust function called from Lua cannot yield, and
//! cannot call a Lua function that may yield, so each step returns to the
//! trampoline with one action, named by its tag: `"yield"` a request and
//! pass every resume value back; call a `"handler"` between
//! `enter_local_handler()` and `leave_local_handler()`, or the
//! `"compactor"`, under the raw `pcall` and pass `ok` and the first
//! result back; `"raise"` a value with `error(value, 0)`; or `"return"`
//! nil. This adapter reads the values the trampoline passes into the
//! machine's typed input, steps the machine, and turns its next step into
//! that action.

use mlua::{Function, Lua, LuaSerdeExt, MultiValue, Table, Value};
use promptforge_types::metrics::ToolCallEvent;

use crate::compactors::OverflowReason;
use crate::error::{Error, Result};
use crate::error_value::{ErrorKind, error_table, normalized};
use crate::hardening::InstructionBudget;
use crate::messages::MessageList;
use crate::protocol::{ChatResult, NOT_A_LIST};

mod machine;

use machine::{Input, Machine, Phase, Then, raised};

/// `models.loop`'s refusal of a model handle in the list's place.
const HANDLE_FIRST: &str = "models.loop takes (messages, compactor?); \
                            call handle:loop(messages, compactor?) to run on a model handle";

/// `models.loop`'s refusal past two arguments.
const LOOP_ARITY: &str = "models.loop takes (messages, compactor?)";

/// A handle's `loop` called without a model handle as its receiver.
const NO_RECEIVER: &str =
    "call loop on a model handle with a colon: handle:loop(messages, compactor?)";

/// A handle's `loop` past its receiver and two arguments.
const METHOD_ARITY: &str = "handle:loop takes (messages, compactor?)";

/// Builds `loop_begin`, the shim chunk's last argument.
///
/// `loop_begin(entry, ...)` takes `false` for `models.loop` or `true` for
/// a handle's `loop`, then that entry's arguments, and returns the
/// call's step closure followed by the first action's tag and values;
/// or `nil`, `"raise"`, and the argument error. The step closure, made
/// with `create_function_mut`, owns the call's [`machine::Machine`]; it
/// takes the last yield's resume values, or the last call's `ok` and
/// first result, and returns the next action's tag and values. mlua
/// refuses a recursive call of such a closure, which cannot happen,
/// because the step returns before every yield and every call. Every
/// error the loop raises is a `"raise"` action, so it is raised from the
/// trampoline's Lua frame; only a crate bug or an exhausted Lua heap
/// makes either function fail.
///
/// `max_tool_iterations` is the round cap, `compactors` the table that
/// `compactors.fail` is read from at each call, and `budget` the VM's
/// instruction budget, whose cancel flag the step reads where the shim
/// called `cancel_requested()`.
///
/// # Errors
/// Returns [`Error::Lua`] if the function cannot be created.
pub(crate) fn loop_begin(
    lua: &Lua,
    max_tool_iterations: usize,
    compactors: Table,
    budget: &InstructionBudget,
) -> Result<Function> {
    let budget = budget.clone();
    lua.create_function(move |lua, args: MultiValue| {
        begin(lua, args, max_tool_iterations, &compactors, &budget)
    })
    .map_err(Error::lua)
}

/// `loop_begin`'s body: the entry flag, then that entry's argument checks
/// in Functional Specification's order, then the machine and its step
/// closure, whose body is `act(lua, machine.step(read_input(lua,
/// machine.phase(), values, &budget)?))`, then the first action.
fn begin(
    lua: &Lua,
    args: MultiValue,
    max_rounds: usize,
    compactors: &Table,
    budget: &InstructionBudget,
) -> mlua::Result<MultiValue> {
    let mut args = args.into_iter();
    let Some(Value::Boolean(method)) = args.next() else {
        return Err(malformed("loop_begin needs its entry flag first"));
    };
    let args: Vec<Value> = args.collect();
    let receiver = args.first().is_some_and(crate::models::is_handle);
    let (handle, mut args) = if method {
        if !receiver {
            return refuse(lua, NO_RECEIVER);
        }
        if args.len() > 3 {
            return refuse(lua, METHOD_ARITY);
        }
        let mut args = args.into_iter();
        (args.next(), args)
    } else {
        if receiver {
            return refuse(lua, HANDLE_FIRST);
        }
        if args.len() > 2 {
            return refuse(lua, LOOP_ARITY);
        }
        (None, args.into_iter())
    };
    let messages = args.next().unwrap_or(Value::Nil);
    let compactor = match args.next().unwrap_or(Value::Nil) {
        Value::Nil => compactors.get::<Value>("fail")?,
        function @ Value::Function(_) => function,
        other => {
            let message = format!(
                "compactor must be a function, got {}",
                lua_type_name(&other)
            );
            return refuse(lua, &message);
        }
    };
    let list = match messages.as_userdata() {
        Some(list) if crate::messages::is_list(&messages) => {
            MessageList::clone(&*list.borrow::<MessageList>()?)
        }
        _ => return refuse(lua, NOT_A_LIST),
    };
    let (mut machine, first) = Machine::begin(max_rounds, list, messages, handle, compactor);
    let budget = budget.clone();
    let step = lua.create_function_mut(move |lua, values: MultiValue| {
        act(
            lua,
            machine.step(read_input(lua, machine.phase(), values, &budget)?),
        )
    })?;
    let mut action = act(lua, first)?;
    action.push_front(Value::Function(step));
    Ok(action)
}

/// `loop_begin`'s answer to a refused argument: no step closure, then the
/// `"raise"` action with a new `lua`-kind error.
fn refuse(lua: &Lua, message: &str) -> mlua::Result<MultiValue> {
    let mut action = act(lua, Then::RaiseNew(raised(ErrorKind::Lua, message)))?;
    action.push_front(Value::Nil);
    Ok(action)
}

/// Reads the values the trampoline passed as the input `phase` awaits.
/// A failed envelope's value goes through [`envelope_failure`]. A `chat`
/// answer is read back into a [`ChatResult`] from the table
/// `chat_result_table` renders: each call's arguments through
/// `from_value`, the overflow tag through `OverflowReason::from_tag`, and
/// `model` and `metrics`, which the loop never reads, left empty. A raw
/// `pcall`'s outcome is read beside the run's cancel flag.
fn read_input(
    lua: &Lua,
    phase: &Phase<Value>,
    values: MultiValue,
    budget: &InstructionBudget,
) -> mlua::Result<Input<Value>> {
    let mut values = values.into_iter();
    let mut next = || values.next().unwrap_or(Value::Nil);
    let ok = truthy(&next());
    let value = next();
    match phase {
        Phase::Compacting | Phase::Handling => Ok(Input::Called {
            outcome: if ok { Ok(value) } else { Err(value) },
            cancelled: budget.is_cancelled(),
        }),
        Phase::Chatting => Ok(Input::Answered(answer(lua, ok, value, |round| {
            chat_result(lua, round).map(Box::new)
        })?)),
        Phase::Calling => {
            let handler = next();
            if ok && !handler.is_nil() {
                return Ok(Input::Local {
                    handler,
                    args: next(),
                });
            }
            Ok(Input::Dispatched(answer(lua, ok, value, text)?))
        }
        Phase::Reporting(_) => Ok(Input::Reported(answer(lua, ok, value, text)?)),
        Phase::Done => Err(malformed("the models.loop step ran after its loop ended")),
    }
}

/// One `(ok, value)` envelope as the machine reads it: `value` read by
/// `read` when `ok`, else the value `fail` raises for it.
fn answer<T>(
    lua: &Lua,
    ok: bool,
    value: Value,
    read: impl FnOnce(Value) -> mlua::Result<T>,
) -> mlua::Result<std::result::Result<T, Value>> {
    if ok {
        read(value).map(Ok)
    } else {
        envelope_failure(lua, value).map(Err)
    }
}

/// Turns the machine's next step into the trampoline's tag and values:
/// each request table built as the shim builds it, a call's arguments
/// converted with `to_value`, the overflow reason as its `tag()`, a
/// normalized failure through `error_value::normalized`, and a new error
/// as `error_table` over `Error::Raised`.
fn act(lua: &Lua, then: Then<Value>) -> mlua::Result<MultiValue> {
    let (tag, values) = match then {
        Then::Chat { messages, handle } => {
            let handle = handle.unwrap_or(Value::Nil);
            let fields = [("messages", messages), ("handle", handle)];
            ("yield", vec![request(lua, "chat", fields)?])
        }
        Then::ToolCall { call, turn } => {
            let fields = [
                ("alias", Value::String(lua.create_string(&call.name)?)),
                ("args", lua.to_value(&call.arguments)?),
                ("call_id", Value::String(lua.create_string(&call.id)?)),
                ("turn", Value::Integer(i64::from(turn))),
            ];
            ("yield", vec![request(lua, "tool_call", fields)?])
        }
        Then::Compact { compactor, reason } => {
            let reason = match reason {
                Some(reason) => Value::String(lua.create_string(reason.tag())?),
                None => Value::Nil,
            };
            ("compactor", vec![compactor, reason])
        }
        Then::Handle { handler, args } => ("handler", vec![handler, args]),
        Then::Report(returned) => {
            let ok = Value::Boolean(returned.is_some());
            let fields = [("ok", ok), ("value", returned.unwrap_or(Value::Nil))];
            ("yield", vec![request(lua, "local_tool_done", fields)?])
        }
        Then::Return => ("return", Vec::new()),
        Then::Raise(value) => ("raise", vec![value]),
        Then::RaiseNormalized(failure) => ("raise", vec![normalized(lua, failure)?]),
        Then::RaiseNew(raised) => {
            let table = error_table(lua, &Error::Raised(raised))?;
            ("raise", vec![Value::Table(table)])
        }
    };
    let mut action = MultiValue::from_vec(values);
    action.push_front(Value::String(lua.create_string(tag)?));
    Ok(action)
}

/// A request table as the shim built it: `op` and the given fields, each
/// nil field left unset.
fn request<const N: usize>(lua: &Lua, op: &str, fields: [(&str, Value); N]) -> mlua::Result<Value> {
    let table = lua.create_table()?;
    table.raw_set("op", op)?;
    for (name, value) in fields {
        table.raw_set(name, value)?;
    }
    Ok(Value::Table(table))
}

/// The value `fail` raises for an envelope's failure: an error table
/// unchanged; anything else a new `lua`-kind table whose message is what
/// the `tostring` global returns for it. The Engine renders every failure
/// as an error table, so only a hand-built envelope reaches the second
/// branch.
fn envelope_failure(lua: &Lua, value: Value) -> mlua::Result<Value> {
    if let Value::Table(_) = value {
        return Ok(value);
    }
    let tostring: Function = lua.globals().get("tostring")?;
    let message: String = tostring.call(value)?;
    let table = error_table(lua, &Error::Raised(raised(ErrorKind::Lua, &message)))?;
    Ok(Value::Table(table))
}

/// Reads a `chat` answer's result table, as `chat_result_table` renders
/// it, back into the [`ChatResult`] the machine judges.
fn chat_result(lua: &Lua, round: Value) -> mlua::Result<ChatResult> {
    let Value::Table(round) = round else {
        return Err(malformed("a chat answer's result is not a table"));
    };
    let overflow_reason = match optional_text(round.raw_get("overflow_reason")?)? {
        Some(tag) => Some(
            OverflowReason::from_tag(&tag)
                .ok_or_else(|| malformed("a chat answer's overflow reason is unknown"))?,
        ),
        None => None,
    };
    let tool_calls = match round.raw_get::<Value>("tool_calls")? {
        Value::Nil => None,
        Value::Table(calls) => Some(
            calls
                .sequence_values::<Table>()
                .map(|call| tool_call(lua, &call?))
                .collect::<mlua::Result<Vec<_>>>()?,
        ),
        _ => return Err(malformed("a chat answer's tool calls are not a table")),
    };
    let turn = match round.raw_get::<Value>("turn")? {
        Value::Integer(turn) => u32::try_from(turn).ok(),
        _ => None,
    };
    Ok(ChatResult {
        overflow: truthy(&round.raw_get("overflow")?),
        overflow_reason,
        reply: optional_text(round.raw_get("reply")?)?,
        empty_detail: optional_text(round.raw_get("empty_detail")?)?,
        tool_calls,
        finish_reason: optional_text(round.raw_get("finish_reason")?)?,
        model: String::new(),
        metrics: None,
        turn: turn.ok_or_else(|| malformed("a chat answer's turn is not a u32 integer"))?,
    })
}

/// One requested call of a `chat` answer's `tool_calls` sequence.
fn tool_call(lua: &Lua, call: &Table) -> mlua::Result<ToolCallEvent> {
    Ok(ToolCallEvent {
        id: text(call.raw_get("id")?)?,
        name: text(call.raw_get("name")?)?,
        arguments: lua.from_value(call.raw_get("arguments")?)?,
        tool: None,
    })
}

/// A Lua string's text; any other value is a malformed answer.
fn text(value: Value) -> mlua::Result<String> {
    match value {
        Value::String(text) => Ok(text.to_str()?.to_owned()),
        _ => Err(malformed("a loop answer's text is not a string")),
    }
}

/// An optional Lua string's text: nil is `None`.
fn optional_text(value: Value) -> mlua::Result<Option<String>> {
    match value {
        Value::Nil => Ok(None),
        value => text(value).map(Some),
    }
}

/// Lua's truth test: everything but nil and false.
fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Nil | Value::Boolean(false))
}

/// The adapter failure for a value the shim chunk or the renderer never
/// produces: a crate bug, raised as an mlua callback error.
fn malformed(what: &'static str) -> mlua::Error {
    mlua::Error::external(Error::Internal(what))
}

/// Lua's `type()` name for `value`, with an integer named `integer`, as
/// the shim's `engine_type` names it. mlua's `type_name` differs for a
/// light userdata (`lightuserdata`) and an mlua error value (`error`),
/// both of which Lua names `userdata`.
fn lua_type_name(value: &Value) -> &'static str {
    match value {
        Value::LightUserData(_) | Value::Error(_) => "userdata",
        other => other.type_name(),
    }
}

#[cfg(test)]
mod tests;
