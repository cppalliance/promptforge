//! The chat request parser: the loop shim's optional leading handle and
//! the author-supplied `messages` list, validated once into message
//! records.

use mlua::{Lua, LuaSerdeExt, Value};

use crate::Error;

use super::super::request::{
    ContentPart, MessageContent, MessageRecord, MessageRole, Request, ToolCallRecord,
};
use super::{FieldFailure, call_handle};

/// The message roles the chat protocol accepts.
const CHAT_ROLES: [&str; 4] = ["system", "user", "assistant", "tool"];

/// The content-part types the chat protocol accepts (the Multimodal
/// contract: text parts and data-URI image parts).
const CHAT_PART_TYPES: [&str; 2] = ["text", "image_url"];

/// Frames one chat author-argument failure as the call's error.
fn chat_error(message: impl Into<String>) -> FieldFailure {
    FieldFailure::Call(Error::Lua(message.into()))
}

/// Parses a `chat` request: the loop shim's optional leading `handle` and
/// the author-supplied `messages` list.
///
/// The whole messages validation happens here, once - the driver converts
/// the validated records without re-checking. Every author-argument
/// failure is the call's error, raised at the `models.loop` call site so a
/// program `pcall` catches it. The handle is checked first, as the loop's
/// leading argument.
pub(super) fn parse_chat(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<Request, FieldFailure> {
    let binding = call_handle(table, "models.loop")?;
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
    let messages = parse_messages(&messages)?;
    Ok(Request::Chat { messages, binding })
}

/// Parses the converted message array into validated records, once, at the
/// protocol boundary: known roles; `content` a string or a non-empty
/// content-parts array with known part types and payloads; a present
/// `tool_call_id` is a string, required on tool entries; a present
/// `tool_calls` is an array of normalized `{id, name, arguments}` records.
/// The empty list is rejected, and every error names the offending 1-based
/// index (the list is Lua-authored). Entry fields beyond the four a record
/// holds (`role`, `content`, `tool_call_id`, `tool_calls`) are accepted
/// and dropped. Cross-record checks - unique call IDs, complete
/// call-result pairing, provider-required alternation - belong to the
/// per-dispatch projection ([`crate::projection`]), not this parse.
fn parse_messages(
    messages: &serde_json::Value,
) -> std::result::Result<Vec<MessageRecord>, FieldFailure> {
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
    entries
        .iter()
        .enumerate()
        .map(|(position, entry)| parse_message(position + 1, entry))
        .collect()
}

/// Parses one message entry into its validated record.
fn parse_message(
    index: usize,
    entry: &serde_json::Value,
) -> std::result::Result<MessageRecord, FieldFailure> {
    let serde_json::Value::Object(entry) = entry else {
        return Err(chat_error(format!(
            "messages[{index}] must be a message table"
        )));
    };
    let role = match entry.get("role") {
        Some(serde_json::Value::String(role)) => match MessageRole::parse(role) {
            Some(role) => role,
            None => {
                return Err(chat_error(format!(
                    "messages[{index}] role {role:?} is unknown; known roles: {}",
                    CHAT_ROLES.join(", ")
                )));
            }
        },
        _ => {
            return Err(chat_error(format!(
                "messages[{index}] role must be a string, one of: {}",
                CHAT_ROLES.join(", ")
            )));
        }
    };
    let content = match entry.get("content") {
        Some(serde_json::Value::String(text)) => MessageContent::Text(text.clone()),
        Some(serde_json::Value::Array(parts)) if !parts.is_empty() => {
            MessageContent::Parts(parse_content_parts(index, parts)?)
        }
        _ => {
            return Err(chat_error(format!(
                "messages[{index}] content must be a string or a non-empty \
                 array of content parts"
            )));
        }
    };
    let tool_call_id = match entry.get("tool_call_id") {
        None => None,
        Some(serde_json::Value::String(id)) => Some(id.clone()),
        Some(_) => {
            return Err(chat_error(format!(
                "messages[{index}] tool_call_id must be a string"
            )));
        }
    };
    if role == MessageRole::Tool && tool_call_id.is_none() {
        return Err(chat_error(format!(
            "messages[{index}] is a tool message and must set a string tool_call_id"
        )));
    }
    let tool_calls = match entry.get("tool_calls") {
        None => Vec::new(),
        Some(serde_json::Value::Array(calls)) => calls
            .iter()
            .enumerate()
            .map(|(position, call)| parse_tool_call_record(index, position + 1, call))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(chat_error(format!(
                "messages[{index}] tool_calls must be an array"
            )));
        }
    };
    Ok(MessageRecord {
        role,
        content,
        tool_calls,
        tool_call_id,
    })
}

/// Parses one message's content-parts array: each part is a table whose
/// `type` names a known part kind, with that kind's required payload.
fn parse_content_parts(
    index: usize,
    parts: &[serde_json::Value],
) -> std::result::Result<Vec<ContentPart>, FieldFailure> {
    parts
        .iter()
        .enumerate()
        .map(|(position, part)| parse_content_part(index, position + 1, part))
        .collect()
}

/// Parses one content part into its typed variant: a `text` part has a
/// string `text` field; an `image_url` part has an `image_url` table
/// with a string `url` field.
fn parse_content_part(
    index: usize,
    part_index: usize,
    part: &serde_json::Value,
) -> std::result::Result<ContentPart, FieldFailure> {
    let malformed = || {
        chat_error(format!(
            "messages[{index}] content part {part_index} must be a table \
             with a string type field"
        ))
    };
    let serde_json::Value::Object(part) = part else {
        return Err(malformed());
    };
    let kind = match part.get("type") {
        Some(serde_json::Value::String(kind)) => kind.as_str(),
        _ => return Err(malformed()),
    };
    match kind {
        "text" => match part.get("text") {
            Some(serde_json::Value::String(text)) => Ok(ContentPart::Text(text.clone())),
            _ => Err(chat_error(format!(
                "messages[{index}] content part {part_index} is a text part \
                 and must set a string text field"
            ))),
        },
        "image_url" => {
            let url = part
                .get("image_url")
                .and_then(serde_json::Value::as_object)
                .and_then(|image| image.get("url"))
                .and_then(serde_json::Value::as_str);
            match url {
                Some(url) => Ok(ContentPart::ImageUrl(url.to_owned())),
                None => Err(chat_error(format!(
                    "messages[{index}] content part {part_index} is an image_url \
                     part and must set an image_url table with a string url field"
                ))),
            }
        }
        unknown => Err(chat_error(format!(
            "messages[{index}] content part {part_index} has unknown type \
             {unknown:?}; known types: {}",
            CHAT_PART_TYPES.join(", ")
        ))),
    }
}

/// Parses one tool call into its normalized record: a string `id`, a
/// string `name`, and an `arguments` object that normalizes to `{}` when
/// absent.
fn parse_tool_call_record(
    index: usize,
    call_index: usize,
    call: &serde_json::Value,
) -> std::result::Result<ToolCallRecord, FieldFailure> {
    let serde_json::Value::Object(call) = call else {
        return Err(chat_error(format!(
            "messages[{index}] tool_calls[{call_index}] must be a table"
        )));
    };
    let id = match call.get("id") {
        Some(serde_json::Value::String(id)) => id.clone(),
        _ => {
            return Err(chat_error(format!(
                "messages[{index}] tool_calls[{call_index}] must set a string id"
            )));
        }
    };
    let name = match call.get("name") {
        Some(serde_json::Value::String(name)) => name.clone(),
        _ => {
            return Err(chat_error(format!(
                "messages[{index}] tool_calls[{call_index}] must set a string name"
            )));
        }
    };
    let arguments = match call.get("arguments") {
        None | Some(serde_json::Value::Null) => serde_json::Value::Object(serde_json::Map::new()),
        Some(arguments @ serde_json::Value::Object(_)) => arguments.clone(),
        Some(_) => {
            return Err(chat_error(format!(
                "messages[{index}] tool_calls[{call_index}] arguments must be a table"
            )));
        }
    };
    Ok(ToolCallRecord {
        id,
        name,
        arguments,
    })
}
