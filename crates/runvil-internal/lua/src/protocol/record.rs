//! The message-record vocabulary a chat request is built from: one
//! validated record, its role, its content, and the tool calls it holds.

use runvil_model_client::client::Role;

use super::non_empty::NonEmpty;

/// One content part of a multimodal message: visible text or a data-URI
/// image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    /// Visible text.
    Text(String),
    /// A data-URI image reference.
    ImageUrl(String),
}

/// A message record's content: plain visible text, or a content-parts
/// array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContent {
    /// Plain visible text.
    Text(String),
    /// A multimodal content-parts array.
    Parts(NonEmpty<ContentPart>),
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

/// A message record's role. A tool result holds the call ID it answers,
/// which only a tool result has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordRole {
    /// A system instruction.
    System,
    /// The user's turn.
    User,
    /// The model's turn.
    Assistant,
    /// A tool result.
    Tool {
        /// The ID of the call the result answers.
        tool_call_id: String,
    },
}

impl RecordRole {
    /// The wire role.
    #[must_use]
    pub fn kind(&self) -> Role {
        match self {
            RecordRole::System => Role::System,
            RecordRole::User => Role::User,
            RecordRole::Assistant => Role::Assistant,
            RecordRole::Tool { .. } => Role::Tool,
        }
    }

    /// The call ID a tool result answers; `None` for every other role.
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            RecordRole::Tool { tool_call_id } => Some(tool_call_id),
            RecordRole::System | RecordRole::User | RecordRole::Assistant => None,
        }
    }
}

/// One validated message record: the plain-message contract every later
/// component (projection, `models.loop`, the message builders) consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRecord {
    /// The message role, with a tool result's call ID.
    pub role: RecordRole,
    /// The visible content.
    pub content: MessageContent,
    /// The normalized tool calls the record holds; empty unless an
    /// assistant turn requested tools.
    pub tool_calls: Vec<ToolCallRecord>,
}

impl MessageRecord {
    /// The record's JSON form: `role`, `content`, and `tool_calls` and
    /// `tool_call_id` when present, in the shapes the chat parse accepts,
    /// so the parse turns it back into an equal record.
    #[must_use]
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        entry.insert("role".to_owned(), self.role.kind().as_str().into());
        entry.insert("content".to_owned(), self.content.to_json());
        if let Some(calls) = self.tool_calls_json() {
            entry.insert("tool_calls".to_owned(), calls);
        }
        if let Some(id) = self.role.tool_call_id() {
            entry.insert("tool_call_id".to_owned(), id.into());
        }
        serde_json::Value::Object(entry)
    }

    /// The `tool_calls` field's JSON form: an array of `{id, name,
    /// arguments}` objects, or `None` when the record holds no calls.
    pub(crate) fn tool_calls_json(&self) -> Option<serde_json::Value> {
        if self.tool_calls.is_empty() {
            return None;
        }
        let calls = self.tool_calls.iter().map(|call| {
            serde_json::json!({
                "id": call.id,
                "name": call.name,
                "arguments": call.arguments,
            })
        });
        Some(calls.collect())
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
