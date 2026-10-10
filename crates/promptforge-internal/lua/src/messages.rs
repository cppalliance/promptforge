//! The `messages` namespace: the Rust-backed message list.
//!
//! `messages.new()` returns a [`MessageList`] userdata. Its chainable
//! `system`/`user`/`assistant`/`tool`/`append` builders and `replace` are
//! colon methods. Each edit validates its records as it adds them, keeps
//! the system records leading the list, and drops fields a record does
//! not hold, so a refused edit raises at the author's call and leaves the
//! list unchanged. `#list` counts the records, `list[i]`, `pairs`, and
//! `ipairs` read them as read-only views with metamethods only, and
//! assignment is refused. `pairs` stops at the record count it started
//! with, as the sandbox `pairs` does for a table, while `ipairs` reads the
//! live list.
//!
//! A model round's request holds a clone of the same list, so the round
//! reads the records the author built with no second validation.

use std::fmt::Display;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use mlua::{
    AnyUserData, Function, LuaSerdeExt, MetaMethod, Table, UserData, UserDataMethods, Value,
    Variadic,
};
use promptforge_model_client::client::Message;
use promptforge_types::ids::RoundId;
use serde_json::{Map, Value as Json};

use super::{Error, Lua, Result};
use crate::protocol::{MessageRecord, MessageRole, parse_record};

#[path = "messages-view.rs"]
mod view;

/// Installs the `messages` global holding `new`, which returns a new
/// empty [`MessageList`].
///
/// # Errors
/// Returns [`Error::Lua`] if the global install fails.
pub(crate) fn install_messages(lua: &Lua, globals: &Table) -> Result<()> {
    let new = lua
        .create_function(|lua, ()| lua.create_userdata(MessageList::default()))
        .map_err(Error::lua)?;
    let messages = lua.create_table().map_err(Error::lua)?;
    messages.raw_set("new", new).map_err(Error::lua)?;
    globals.raw_set("messages", messages).map_err(Error::lua)
}

/// Whether `value` is a `messages.new()` list.
pub(crate) fn is_list(value: &Value) -> bool {
    matches!(value, Value::UserData(userdata) if userdata.is::<MessageList>())
}

/// A `messages.new()` list. The same value is the Lua userdata and the
/// handle a model round's request holds; clones share one list.
#[derive(Clone, Debug, Default)]
pub struct MessageList(Arc<Mutex<ListState>>);

/// What a list holds.
#[derive(Debug, Default)]
struct ListState {
    /// The records, in order, shared with the views read from them.
    records: Vec<Arc<MessageRecord>>,
    /// The last issued send: its round and the wire request it sent.
    last: Option<(RoundId, Vec<Message>)>,
}

impl MessageList {
    /// The list's state. Every edit swaps in its result whole, so a lock
    /// poisoned by a panic elsewhere still guards a valid list.
    fn state(&self) -> MutexGuard<'_, ListState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The records in order, for a round's projection.
    #[must_use]
    pub fn records(&self) -> Vec<MessageRecord> {
        self.state()
            .records
            .iter()
            .map(|record| MessageRecord::clone(record))
            .collect()
    }

    /// How many records the list holds.
    fn len(&self) -> usize {
        self.state().records.len()
    }

    /// The record at the 1-based `index`, if the list holds one there.
    fn record(&self, index: usize) -> Option<Arc<MessageRecord>> {
        let position = index.checked_sub(1)?;
        self.state().records.get(position).cloned()
    }

    /// Appends `record`. A system record after any non-system record is
    /// refused with a message naming its position.
    ///
    /// # Errors
    /// Returns the refusal's message for a late system record.
    pub fn push(&self, record: MessageRecord) -> std::result::Result<(), String> {
        let mut state = self.state();
        if record.role == MessageRole::System
            && state
                .records
                .iter()
                .any(|held| held.role != MessageRole::System)
        {
            return Err(late_system(state.records.len() + 1));
        }
        state.records.push(Arc::new(record));
        Ok(())
    }

    /// Replaces the 1-based inclusive range `first..=last` with
    /// `records`, where `1 <= first <= last + 1 <= len + 1`, then checks
    /// the leading-system rule. A refused edit leaves the list unchanged.
    fn replace(
        &self,
        first: usize,
        last: usize,
        records: Vec<MessageRecord>,
    ) -> std::result::Result<(), String> {
        let mut state = self.state();
        let len = state.records.len();
        if !in_bounds(first, last, len) {
            return Err(out_of_bounds(first, last, len));
        }
        let edited: Vec<Arc<MessageRecord>> = state.records[..first - 1]
            .iter()
            .cloned()
            .chain(records.into_iter().map(Arc::new))
            .chain(state.records[last..].iter().cloned())
            .collect();
        check_leading_system(&edited)?;
        state.records = edited;
        Ok(())
    }

    /// Records `request` as the list's send in `round`, and returns the
    /// previous send's round and how many leading messages `request`
    /// shares with that send's request; `(None, 0)` on the first send.
    #[must_use]
    pub fn commit(&self, round: RoundId, request: &[Message]) -> (Option<RoundId>, u64) {
        let previous = self.state().last.replace((round, request.to_vec()));
        previous.map_or((None, 0), |(after, sent)| {
            let keep = sent
                .iter()
                .zip(request)
                .take_while(|(sent, now)| sent == now)
                .count();
            (Some(after), u64::try_from(keep).unwrap_or(u64::MAX))
        })
    }
}

/// Whether `replace` can edit `first..=last` on a list of `len` records:
/// `1 <= first <= last + 1 <= len + 1`.
fn in_bounds(first: usize, last: usize, len: usize) -> bool {
    first >= 1 && last <= len && first <= last + 1
}

/// The refusal for a `replace` range outside the list's bounds.
fn out_of_bounds(first: impl Display, last: impl Display, len: usize) -> String {
    format!(
        "replace({first}, {last}) is out of bounds on a list of {len} records: it needs \
         1 <= first <= last + 1 <= {}",
        len + 1
    )
}

/// Refuses records whose system records do not all lead them, naming the
/// first late one.
fn check_leading_system(records: &[Arc<MessageRecord>]) -> std::result::Result<(), String> {
    let lead = records
        .iter()
        .take_while(|record| record.role == MessageRole::System)
        .count();
    match records[lead..]
        .iter()
        .position(|record| record.role == MessageRole::System)
    {
        Some(offset) => Err(late_system(lead + offset + 1)),
        None => Ok(()),
    }
}

/// The refusal for a system record at `index`, behind a non-system one.
fn late_system(index: usize) -> String {
    format!(
        "messages[{index}] is a system message after a non-system message; system \
         messages lead the list, so change the leading block with \
         replace(first, last, records...)"
    )
}

/// Raises `message` as a list method's error, which an author `pcall`
/// catches.
fn refusal(message: String) -> mlua::Error {
    mlua::Error::external(Error::Lua(message))
}

/// The list a method was called on.
fn handle(this: &AnyUserData) -> mlua::Result<MessageList> {
    Ok(this.borrow::<MessageList>()?.clone())
}

/// One Lua value in its JSON form, or why it has none.
fn json(lua: &Lua, value: Value) -> std::result::Result<Json, String> {
    lua.from_value(value).map_err(|error| match error {
        mlua::Error::DeserializeError(reason) => reason,
        other => other.to_string(),
    })
}

/// A whole record given to `append` or `replace`, in its JSON form, for
/// the position `index` it takes.
fn record_json(lua: &Lua, index: usize, value: Value) -> mlua::Result<Json> {
    json(lua, value).map_err(|reason| {
        refusal(format!(
            "messages[{index}] must be a JSON-representable message table: {reason}"
        ))
    })
}

/// Validates `entry` as the record at `index`, the end of `list`, and
/// appends it.
fn add(list: &MessageList, index: usize, entry: &Json) -> mlua::Result<()> {
    let record = parse_record(index, entry).map_err(refusal)?;
    list.push(record).map_err(refusal)
}

/// Appends the record a builder makes from `role` and `fields`, leaving
/// out each field given as nil, and returns the list for chaining.
fn build<const N: usize>(
    lua: &Lua,
    this: AnyUserData,
    role: &str,
    fields: [(&str, Value); N],
) -> mlua::Result<AnyUserData> {
    let list = handle(&this)?;
    let index = list.len() + 1;
    let mut entry = Map::new();
    entry.insert("role".to_owned(), Json::from(role));
    for (name, value) in fields {
        if !value.is_nil() {
            let value = json(lua, value).map_err(|reason| {
                refusal(format!(
                    "messages[{index}] {name} must be JSON-representable: {reason}"
                ))
            })?;
            entry.insert(name.to_owned(), value);
        }
    }
    add(&list, index, &Json::Object(entry))?;
    Ok(this)
}

/// A Lua number under the integer rule: an integer, or a float with an
/// integral value.
fn integral(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(integer) => Some(*integer),
        Value::Number(number) if number.fract() == 0.0 =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the value is integral, and a magnitude past i64 saturates to a value \
                          that is out of range either way"
            )]
            Some(*number as i64)
        }
        _ => None,
    }
}

/// The 1-based list index a Lua key names under the integer rule.
fn key_index(key: &Value) -> Option<usize> {
    integral(key).and_then(|index| usize::try_from(index).ok())
}

/// One `replace` bound under the integer rule.
fn bound(name: &str, value: &Value) -> mlua::Result<i64> {
    integral(value).ok_or_else(|| match value {
        Value::Number(bound) => refusal(format!("replace {name} must be an integer, got {bound}")),
        other => refusal(format!(
            "replace {name} must be an integer, got {}",
            other.type_name()
        )),
    })
}

/// The index after `previous` in a `pairs` walk, and a new view of the
/// live record there, or nils past `extent` or past the live list's end.
/// `extent` is the record count when the walk began, so a loop that
/// appends still ends, as the sandbox `pairs` fixes a table's keys at its
/// start.
fn next_record(
    lua: &Lua,
    list: &MessageList,
    previous: &Value,
    extent: usize,
) -> mlua::Result<(Option<usize>, Value)> {
    let index = match previous {
        Value::Nil => Some(1),
        key => key_index(key).and_then(|index| index.checked_add(1)),
    };
    match index
        .filter(|index| *index <= extent)
        .and_then(|index| Some((index, list.record(index)?)))
    {
        Some((index, record)) => Ok((Some(index), view::create(lua, record)?)),
        None => Ok((None, Value::Nil)),
    }
}

impl UserData for MessageList {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("system", |lua, (this, content): (AnyUserData, Value)| {
            build(lua, this, "system", [("content", content)])
        });
        methods.add_function("user", |lua, (this, content): (AnyUserData, Value)| {
            build(lua, this, "user", [("content", content)])
        });
        methods.add_function(
            "assistant",
            |lua, (this, content, tool_calls): (AnyUserData, Value, Option<Value>)| {
                let tool_calls = tool_calls.unwrap_or(Value::Nil);
                build(
                    lua,
                    this,
                    "assistant",
                    [("content", content), ("tool_calls", tool_calls)],
                )
            },
        );
        methods.add_function(
            "tool",
            |lua, (this, content, tool_call_id): (AnyUserData, Value, Value)| {
                build(
                    lua,
                    this,
                    "tool",
                    [("content", content), ("tool_call_id", tool_call_id)],
                )
            },
        );
        methods.add_function("append", |lua, (this, record): (AnyUserData, Value)| {
            let list = handle(&this)?;
            let index = list.len() + 1;
            add(&list, index, &record_json(lua, index, record)?)?;
            Ok(this)
        });
        methods.add_function(
            "replace",
            |lua, (this, first, last, records): (AnyUserData, Value, Value, Variadic<Value>)| {
                let target = handle(&this)?;
                let first = bound("first", &first)?;
                let last = bound("last", &last)?;
                let len = target.len();
                let (first, last) = match (usize::try_from(first), usize::try_from(last)) {
                    (Ok(first), Ok(last)) if in_bounds(first, last, len) => (first, last),
                    _ => return Err(refusal(out_of_bounds(first, last, len))),
                };
                let records = records
                    .into_iter()
                    .enumerate()
                    .map(|(offset, value)| {
                        let index = first + offset;
                        parse_record(index, &record_json(lua, index, value)?).map_err(refusal)
                    })
                    .collect::<mlua::Result<Vec<_>>>()?;
                target.replace(first, last, records).map_err(refusal)?;
                Ok(this)
            },
        );
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.len()));
        methods.add_meta_method(
            MetaMethod::Index,
            |lua, this, key: Value| match key_index(&key).and_then(|index| this.record(index)) {
                Some(record) => view::create(lua, record),
                None => Ok(Value::Nil),
            },
        );
        methods.add_meta_function(
            MetaMethod::Pairs,
            |lua, this: AnyUserData| -> mlua::Result<(Function, AnyUserData, Value)> {
                let list = handle(&this)?;
                let extent = list.len();
                let next = lua.create_function(move |lua, (_, previous): (Value, Value)| {
                    next_record(lua, &list, &previous, extent)
                })?;
                Ok((next, this, Value::Nil))
            },
        );
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, _: (Value, Value)| -> mlua::Result<()> {
                Err(refusal(
                    "a messages.new() list cannot be assigned to; add records with \
                     append(record) and change them with replace(first, last, records...)"
                        .to_owned(),
                ))
            },
        );
    }
}

#[cfg(test)]
#[path = "messages-tests.rs"]
mod tests;
