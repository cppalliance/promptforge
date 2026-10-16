//! Shared value decoding for the `tools.*` Engine functions.
//!
//! The id-or-tool-object polymorphism lives here once: `tools.offer` and
//! the `tools.call` protocol parse both accept a bare string or a tool
//! object and resolve each to the name it stands for, a tool object
//! standing for its canonical id. The
//! `tools.offer_local` params-table-to-JSON-Schema conversion sits beside
//! it as the namespace's other argument decode.

use mlua::{Value, Variadic};
use serde_json::{Value as Json, json};

use super::userdata::LuaToolHandle;

/// Resolves one string or tool object to the name it stands for.
///
/// This is the single decode behind the namespace's argument polymorphism:
/// a string is the name verbatim (a tool id, or for `tools.call` a local
/// alias), a tool object contributes its canonical id, and anything else
/// is an argument error naming the accepted forms.
///
/// # Errors
/// Returns an `mlua` external error when the value is neither a string nor
/// a tool object.
pub(crate) fn tool_name(value: &Value) -> mlua::Result<String> {
    let rejected = || {
        mlua::Error::external(format!(
            "tools.call takes a tool id, a local alias, or a tool object, got {}",
            value.type_name()
        ))
    };
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        // A userdata that is not a tool object (a model handle) takes the
        // same rejection as any other wrong type.
        Value::UserData(ud) => ud
            .borrow::<LuaToolHandle>()
            .map(|handle| handle.id().to_string())
            .map_err(|_| rejected()),
        _ => Err(rejected()),
    }
}

/// Reads one element of `call`'s arguments as a name: a string or a tool
/// object.
fn entry_name(call: &str, value: &Value) -> mlua::Result<String> {
    tool_name(value).map_err(|_| {
        mlua::Error::external(format!(
            "{call} takes tool ids, tool objects, or arrays of them, got {}",
            value.type_name()
        ))
    })
}

/// Flattens the arguments of `call` (`tools.offer`) into the names they
/// stand for, naming `call` in every argument-shape error.
///
/// `tools.offer(tool)` takes one tool id or tool object. A table is the
/// array form, `tools.offer({"web/search", "web/fetch"})` or
/// `tools.offer(tools.extras())`, which covers bulk. Any second argument is
/// refused.
pub(super) fn collect_offer_entries(
    call: &str,
    args: Variadic<Value>,
) -> mlua::Result<Vec<String>> {
    let mut args = args.into_iter();
    let Some(target) = args.next() else {
        return Ok(Vec::new());
    };
    if let Some(extra) = args.next() {
        return Err(mlua::Error::external(format!(
            "{call} takes one tool or a list of them, got extra {}",
            extra.type_name()
        )));
    }
    match target {
        Value::Table(table) => table
            .sequence_values::<Value>()
            .map(|item| entry_name(call, &item?))
            .collect(),
        single => Ok(vec![entry_name(call, &single)?]),
    }
}

/// Builds the JSON Schema `parameters` object from a `tools.offer_local` params
/// table. Each value is a bare type string or a `{type, description}` array;
/// every declared parameter is required.
///
/// The `required` list is the only Rust-side `table.pairs` walk in this crate
/// that builds an ordered array without sorting it, so it sorts its names
/// itself. The other ordered walks, `ordered_keys` in `iteration.rs` and
/// `collection_members` in `collection.rs`, already sort their output. The
/// walk's order is unspecified, so a schema left in it would read differently
/// in two VMs; `properties` needs no sort because it is a `serde_json::Map`,
/// a `BTreeMap` with no `preserve_order` in the graph.
pub(super) fn local_params_schema(params: &mlua::Table) -> mlua::Result<Json> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for pair in params.pairs::<String, Value>() {
        let (name, spec) = pair?;
        let (ty, description) = match spec {
            Value::String(s) => (s.to_string_lossy(), None),
            Value::Table(t) => (t.get::<String>(1)?, t.get::<Option<String>>(2)?),
            _ => {
                return Err(mlua::Error::external(format!(
                    "tools.offer_local param {name:?} must be a type string or a {{type, description}} array"
                )));
            }
        };
        if !matches!(ty.as_str(), "string" | "integer" | "number" | "boolean") {
            return Err(mlua::Error::external(format!(
                "tools.offer_local param {name:?} has unsupported type {ty:?}: \
                 expected \"string\", \"integer\", \"number\", or \"boolean\""
            )));
        }
        let mut property = json!({ "type": ty });
        if let Some(description) = description {
            property["description"] = Json::String(description);
        }
        properties.insert(name.clone(), property);
        required.push(name);
    }
    // Bytewise sort, matching the shared `SortKey::Text` order, so the
    // generated schema is the same text in every process.
    required.sort();
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
}
