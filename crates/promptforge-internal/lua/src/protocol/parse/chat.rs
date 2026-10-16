//! The chat request parser: the handle a handle's `loop` names and the
//! author's `messages.new()` list, plus the per-record validation the
//! list's builders run as they add a record.

use mlua::Value;
use promptforge_model_client::client::Role;

use crate::Error;
use crate::messages::MessageList;
use crate::protocol::{
    ContentPart, MessageContent, MessageRecord, NonEmpty, RecordRole, Request, ToolCallRecord,
};

use super::{FieldFailure, call_handle};

/// The refusal for a `messages` value that is not a `messages.new()`
/// list. The chat parse and the loop's argument check share it.
pub(crate) const NOT_A_LIST: &str = "models.loop needs a messages.new() list; build one with \
                                     messages.new() and :user, :append, or :replace";

/// The message roles the chat protocol accepts, comma-separated.
fn chat_roles() -> String {
    Role::ALL
        .iter()
        .map(|role| role.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The content-part types the chat protocol accepts (the Multimodal
/// contract: text parts and data-URI image parts).
const CHAT_PART_TYPES: [&str; 2] = ["text", "image_url"];

/// Frames one chat author-argument failure as the call's error.
fn chat_error(message: impl Into<String>) -> FieldFailure {
    FieldFailure::Call(Error::Lua(message.into()))
}

/// Parses a `chat` request: the optional `handle`, which a handle's
/// `loop` sets from its receiver, and the `messages` list, which must be a
/// `messages.new()` list.
///
/// The list validated each record as it added it, so the parse checks
/// only that `messages` is such a list and clones its handle; the driver
/// projects the records without re-checking them. An empty list parses:
/// the driver refuses it after pushing the round's model-task notices,
/// which can fill it. Every author-argument failure is the call's error,
/// raised at the `models.loop` call site so a program `pcall` catches it.
/// The handle is checked first.
pub(super) fn parse_chat(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let binding = call_handle(table, "models.loop")?;
    let list = match table.raw_get::<Value>("messages") {
        Ok(Value::UserData(userdata)) => userdata
            .borrow::<MessageList>()
            .map(|list| MessageList::clone(&list))
            .ok(),
        Ok(_) => None,
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let list = list.ok_or_else(|| chat_error(NOT_A_LIST))?;
    Ok(Request::Chat { list, binding })
}

/// Validates one record in its JSON form, as a list builder adds it.
/// `index` is the 1-based position the record takes, named in the
/// error.
///
/// The rules: a known role; `content` a string or a non-empty
/// content-parts array with known part types and payloads; a
/// `tool_call_id` a string, required on tool records and refused on every
/// other; a present `tool_calls` an array of normalized
/// `{id, name, arguments}` records. Fields beyond the four a record holds
/// (`role`, `content`, `tool_call_id`, `tool_calls`) are dropped.
/// Cross-record checks - unique call IDs, complete call-result pairing,
/// provider-required alternation - belong to the per-dispatch projection
/// ([`crate::projection`]).
pub(crate) fn parse_record(
    index: usize,
    entry: &serde_json::Value,
) -> std::result::Result<MessageRecord, String> {
    parse_message(index, entry)
}

/// Parses one message entry into its validated record.
fn parse_message(
    index: usize,
    entry: &serde_json::Value,
) -> std::result::Result<MessageRecord, String> {
    let serde_json::Value::Object(entry) = entry else {
        return Err(format!("messages[{index}] must be a message table"));
    };
    let role = match entry.get("role") {
        Some(serde_json::Value::String(role)) => match role.parse::<Role>() {
            Ok(role) => role,
            Err(_) => {
                return Err(format!(
                    "messages[{index}] role {role:?} is unknown; known roles: {}",
                    chat_roles()
                ));
            }
        },
        _ => {
            return Err(format!(
                "messages[{index}] role must be a string, one of: {}",
                chat_roles()
            ));
        }
    };
    let content = match entry.get("content") {
        Some(serde_json::Value::String(text)) => MessageContent::Text(text.clone()),
        Some(serde_json::Value::Array(parts)) => {
            match NonEmpty::from_vec(parse_content_parts(index, parts)?) {
                Some(parts) => MessageContent::Parts(parts),
                None => return Err(content_refusal(index)),
            }
        }
        _ => return Err(content_refusal(index)),
    };
    let tool_call_id = match entry.get("tool_call_id") {
        None => None,
        Some(serde_json::Value::String(id)) => Some(id.clone()),
        Some(_) => {
            return Err(format!("messages[{index}] tool_call_id must be a string"));
        }
    };
    let role = match (role, tool_call_id) {
        (Role::Tool, Some(tool_call_id)) => RecordRole::Tool { tool_call_id },
        (Role::Tool, None) => {
            return Err(format!(
                "messages[{index}] is a tool message and must set a string tool_call_id"
            ));
        }
        (_, Some(_)) => {
            return Err(format!(
                "messages[{index}] sets a tool_call_id but is not a tool message"
            ));
        }
        (Role::System, None) => RecordRole::System,
        (Role::User, None) => RecordRole::User,
        (Role::Assistant, None) => RecordRole::Assistant,
        (other, None) => {
            return Err(format!(
                "messages[{index}] role {:?} is unknown; known roles: {}",
                other.as_str(),
                chat_roles()
            ));
        }
    };
    let tool_calls = match entry.get("tool_calls") {
        None => Vec::new(),
        Some(serde_json::Value::Array(calls)) => calls
            .iter()
            .enumerate()
            .map(|(position, call)| parse_tool_call_record(index, position + 1, call))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(format!("messages[{index}] tool_calls must be an array"));
        }
    };
    Ok(MessageRecord {
        role,
        content,
        tool_calls,
    })
}

/// The refusal for a `content` that is neither a string nor a non-empty
/// content-parts array.
fn content_refusal(index: usize) -> String {
    format!("messages[{index}] content must be a string or a non-empty array of content parts")
}

/// Parses one message's content-parts array: each part is a table whose
/// `type` names a known part kind, with that kind's required payload.
fn parse_content_parts(
    index: usize,
    parts: &[serde_json::Value],
) -> std::result::Result<Vec<ContentPart>, String> {
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
) -> std::result::Result<ContentPart, String> {
    let malformed = || {
        format!(
            "messages[{index}] content part {part_index} must be a table \
             with a string type field"
        )
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
            _ => Err(format!(
                "messages[{index}] content part {part_index} is a text part \
                 and must set a string text field"
            )),
        },
        "image_url" => {
            let Some(serde_json::Value::Object(image)) = part.get("image_url") else {
                return Err(image_url_refusal(index, part_index));
            };
            match image.get("url") {
                Some(serde_json::Value::String(url)) => Ok(ContentPart::ImageUrl(url.clone())),
                _ => Err(image_url_refusal(index, part_index)),
            }
        }
        unknown => Err(format!(
            "messages[{index}] content part {part_index} has unknown type \
             {unknown:?}; known types: {}",
            CHAT_PART_TYPES.join(", ")
        )),
    }
}

/// The refusal for an `image_url` part without an `image_url` table
/// holding a string `url`.
fn image_url_refusal(index: usize, part_index: usize) -> String {
    format!(
        "messages[{index}] content part {part_index} is an image_url part and must set an \
         image_url table with a string url field"
    )
}

/// Parses one tool call into its normalized record: a string `id`, a
/// string `name`, and an `arguments` object that normalizes to `{}` when
/// absent.
fn parse_tool_call_record(
    index: usize,
    call_index: usize,
    call: &serde_json::Value,
) -> std::result::Result<ToolCallRecord, String> {
    let serde_json::Value::Object(call) = call else {
        return Err(format!(
            "messages[{index}] tool_calls[{call_index}] must be a table"
        ));
    };
    let id = match call.get("id") {
        Some(serde_json::Value::String(id)) => id.clone(),
        _ => {
            return Err(format!(
                "messages[{index}] tool_calls[{call_index}] must set a string id"
            ));
        }
    };
    let name = match call.get("name") {
        Some(serde_json::Value::String(name)) => name.clone(),
        _ => {
            return Err(format!(
                "messages[{index}] tool_calls[{call_index}] must set a string name"
            ));
        }
    };
    let arguments = match call.get("arguments") {
        None | Some(serde_json::Value::Null) => serde_json::Value::Object(serde_json::Map::new()),
        Some(arguments @ serde_json::Value::Object(_)) => arguments.clone(),
        Some(_) => {
            return Err(format!(
                "messages[{index}] tool_calls[{call_index}] arguments must be a table"
            ));
        }
    };
    Ok(ToolCallRecord {
        id,
        name,
        arguments,
    })
}
