//! Read-only views of a list's records, which `msgs[i]` and iteration
//! return.
//!
//! A view holds its record by `Arc`, so reading one copies nothing until a
//! field is read. Table fields are built fresh on every read, so writing
//! into them never reaches the record.

use std::sync::Arc;

use mlua::{AnyUserData, Function, Lua, LuaSerdeExt, MetaMethod, UserData, UserDataMethods, Value};

use super::refusal;
use crate::protocol::MessageRecord;

/// The fields a view reads, in the order `pairs` visits them.
const FIELDS: [&str; 4] = ["role", "content", "tool_calls", "tool_call_id"];

/// A read-only view of one record, which `msgs[i]` and iteration return.
/// It serializes as the record's JSON form, so a view converts like the
/// record table it stands for.
#[derive(Clone, Debug)]
struct RecordView(Arc<MessageRecord>);

/// A new view of `record`.
pub(super) fn create(lua: &Lua, record: Arc<MessageRecord>) -> mlua::Result<Value> {
    lua.create_ser_userdata(RecordView(record))
        .map(Value::UserData)
}

impl serde::Serialize for RecordView {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.to_json().serialize(serializer)
    }
}

/// The position in [`FIELDS`] of the field `key` names, if it names one.
fn field_position(key: &Value) -> Option<usize> {
    let Value::String(name) = key else {
        return None;
    };
    let name = name.as_bytes();
    FIELDS.iter().position(|field| field.as_bytes() == &*name)
}

/// The field `name` of `record`, or nil when the record has none.
fn field(lua: &Lua, record: &MessageRecord, name: &str) -> mlua::Result<Value> {
    match name {
        "role" => lua.to_value(record.role.as_str()),
        "content" => lua.to_value(&record.content.to_json()),
        "tool_calls" => match record.tool_calls_json() {
            Some(calls) => lua.to_value(&calls),
            None => Ok(Value::Nil),
        },
        "tool_call_id" => match &record.tool_call_id {
            Some(id) => lua.to_value(id),
            None => Ok(Value::Nil),
        },
        _ => Ok(Value::Nil),
    }
}

/// The first field of `record` present after `previous` in [`FIELDS`]
/// order, with its value, or nils past the last.
fn next_field(
    lua: &Lua,
    record: &MessageRecord,
    previous: &Value,
) -> mlua::Result<(Option<&'static str>, Value)> {
    let start = match previous {
        Value::Nil => 0,
        key => field_position(key).map_or(FIELDS.len(), |position| position + 1),
    };
    for name in &FIELDS[start..] {
        let value = field(lua, record, name)?;
        if !value.is_nil() {
            return Ok((Some(*name), value));
        }
    }
    Ok((None, Value::Nil))
}

impl UserData for RecordView {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(
            MetaMethod::Index,
            |lua, this, key: Value| match field_position(&key) {
                Some(position) => field(lua, &this.0, FIELDS[position]),
                None => Ok(Value::Nil),
            },
        );
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, _: (Value, Value)| -> mlua::Result<()> {
                Err(refusal(
                    "records read from a messages.new() list are read-only; change the list \
                     with replace(first, last, records...)"
                        .to_owned(),
                ))
            },
        );
        methods.add_meta_function(
            MetaMethod::Pairs,
            |lua, this: AnyUserData| -> mlua::Result<(Function, AnyUserData, Value)> {
                let record = Arc::clone(&this.borrow::<RecordView>()?.0);
                let next = lua.create_function(move |lua, (_, previous): (Value, Value)| {
                    next_field(lua, &record, &previous)
                })?;
                Ok((next, this, Value::Nil))
            },
        );
    }
}
