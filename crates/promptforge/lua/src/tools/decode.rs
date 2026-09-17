//! Shared value decoding for the `tools.*` host tables.
//!
//! The alias-or-Tool polymorphism lives here once: `tools.add`
//! and the `tools.call` protocol parse both accept a bare
//! alias string or an inspectable Tool object and resolve either to the
//! prompt-local alias. The `tools.add_local` params-table-to-JSON-Schema
//! conversion sits beside it as the namespace's other argument decode.

use mlua::{Value, Variadic};
use serde_json::{Value as Json, json};

use super::userdata::LuaToolHandle;

/// Resolves one alias-or-Tool value to its prompt-local alias.
///
/// This is the single decode behind the namespace's argument polymorphism:
/// a string is the alias verbatim, a Tool object contributes the alias it
/// was bound under, and anything else is an argument error naming the two
/// accepted forms.
///
/// # Errors
/// Returns an `mlua` external error when the value is neither a string nor
/// a Tool object.
pub(crate) fn tool_alias(value: &Value) -> mlua::Result<String> {
    let rejected = || {
        mlua::Error::external(format!(
            "tools.call alias must be a string or Tool object, got {}",
            value.type_name()
        ))
    };
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        // A userdata that is not a Tool object (a model handle, a fanout
        // result) takes the same rejection as any other wrong type.
        Value::UserData(ud) => ud
            .borrow::<LuaToolHandle>()
            .map(|handle| handle.name().to_owned())
            .map_err(|_| rejected()),
        _ => Err(rejected()),
    }
}

/// One flattened `tools.add` entry: alias plus optional model-description override.
pub(crate) struct ToolsAddEntry {
    pub(crate) alias: String,
    pub(crate) description_override: Option<String>,
}

/// Reads one `tools.add` element as an alias: a string or a Tool handle.
fn add_alias(value: &Value) -> mlua::Result<String> {
    tool_alias(value).map_err(|_| {
        mlua::Error::external(format!(
            "tools.add expects strings, Tool objects, or arrays of either, got {}",
            value.type_name()
        ))
    })
}

/// Flattens the `tools.add` arguments into alias/override entries.
///
/// `tools.add(alias, override?)` takes one alias (string or Tool handle) with
/// an optional model-description override. The array form
/// `tools.add({"a", "b"})` covers bulk and takes no per-element overrides.
pub(crate) fn collect_tools_add_entries(args: Variadic<Value>) -> mlua::Result<Vec<ToolsAddEntry>> {
    let mut args = args.into_iter();
    let Some(target) = args.next() else {
        return Ok(Vec::new());
    };
    let description_override = match args.next() {
        None => None,
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        Some(other) => {
            return Err(mlua::Error::external(format!(
                "tools.add override must be a string, got {}",
                other.type_name()
            )));
        }
    };
    if let Some(extra) = args.next() {
        return Err(mlua::Error::external(format!(
            "tools.add takes one alias plus an optional override, got extra {}",
            extra.type_name()
        )));
    }
    match target {
        Value::Table(table) => {
            if description_override.is_some() {
                return Err(mlua::Error::external(
                    "tools.add array form takes no override",
                ));
            }
            table
                .sequence_values::<Value>()
                .map(|item| {
                    Ok(ToolsAddEntry {
                        alias: add_alias(&item?)?,
                        description_override: None,
                    })
                })
                .collect()
        }
        single => Ok(vec![ToolsAddEntry {
            alias: add_alias(&single)?,
            description_override,
        }]),
    }
}

/// Builds the JSON Schema `parameters` object from a `tools.add_local` params
/// table. Each value is a bare type string or a `{type, description}` array;
/// every declared parameter is required.
pub(crate) fn add_local_params_schema(params: &mlua::Table) -> mlua::Result<Json> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for pair in params.pairs::<String, Value>() {
        let (name, spec) = pair?;
        let (ty, description) = match spec {
            Value::String(s) => (s.to_string_lossy(), None),
            Value::Table(t) => (t.get::<String>(1)?, t.get::<Option<String>>(2)?),
            _ => {
                return Err(mlua::Error::external(format!(
                    "tools.add_local param {name:?} must be a type string or a {{type, description}} array"
                )));
            }
        };
        if !matches!(ty.as_str(), "string" | "integer" | "number" | "boolean") {
            return Err(mlua::Error::external(format!(
                "tools.add_local param {name:?} has unsupported type {ty:?}: \
                 expected \"string\", \"integer\", \"number\", or \"boolean\""
            )));
        }
        let mut property = json!({ "type": ty });
        if let Some(description) = description {
            property["description"] = Json::String(description);
        }
        properties.insert(name.clone(), property);
        required.push(Json::String(name));
    }
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
}
