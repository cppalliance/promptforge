//! The per-dispatch context projection: cross-record validation and
//! provider-shape healing over a validated message list.
//!
//! The protocol parse ([`crate::protocol`]) validates each message record's
//! own fields once, at the yield boundary. Everything that is a property of
//! the list rather than of one record lives here and runs immediately before
//! dispatch, on every model call, over whichever records the author's list
//! holds at that moment:
//!
//! - `tool_calls` belong to assistant records and `tool_call_id` to tool
//!   records; a system message is legal only in the leading block.
//! - Tool-call IDs are unique across the whole list, and every assistant
//!   tool-call batch is answered atomically: exactly the records immediately
//!   following the batch are its results, one per call, each exactly once.
//! - Provider-required alternation is healed, not rejected: multiple leading
//!   system messages compose into one (without mutating the source array),
//!   consecutive same-role user records coalesce into one message, and
//!   consecutive text-only assistant records - streaming fragments an author
//!   appended as they arrived - coalesce into one assistant result.
//! - Metadata is stripped: the wire message carries exactly `role`,
//!   `content`, `tool_call_id`, and `tool_calls`, so anything an author
//!   record carried beyond the contract (a copied credential, say) can never
//!   reach the provider.
//!
//! The output is the provider-neutral wire shape the gateway speaks; the
//! projection is recomputed per dispatch for whichever model the call
//! targets, so a later provider-specific mapping changes this one module.

use std::collections::BTreeSet;

use promptforge_model_client::client::Message;
use serde_json::Value;

use crate::Error;
use crate::Result;
use crate::protocol::{ContentPart, MessageContent, MessageRecord, MessageRole};

/// Validates and projects one author-built message list into the wire
/// messages for one model dispatch.
///
/// # Errors
/// Returns [`Error::Lua`] naming the offending 1-based index when the list
/// violates a cross-record rule: a malformed record (tool calls on a
/// non-assistant, a call ID on a non-tool, a system message outside the
/// leading block, content parts in a system block that must be composed), a
/// duplicate tool-call ID, an orphan tool record, or an assistant tool call
/// with no result.
pub fn project_messages(messages: &[MessageRecord]) -> Result<Vec<Message>> {
    validate(messages)?;
    Ok(project(messages))
}

/// The cross-record validation pass: every rule that rejects a list, each
/// error naming the offending 1-based index (the list is Lua-authored).
fn validate(messages: &[MessageRecord]) -> Result<()> {
    let system_len = leading_system_len(messages);
    if system_len > 1 {
        for (position, record) in messages[..system_len].iter().enumerate() {
            if matches!(record.content, MessageContent::Parts(_)) {
                return Err(Error::Lua(format!(
                    "messages[{}] is a system message with content parts; only plain \
                     text system messages can be composed",
                    position + 1
                )));
            }
        }
    }
    // Every call ID ever seen: uniqueness holds across the whole list, not
    // only within one batch.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    // The open batch: the assistant record's 1-based index and its still
    // unanswered call IDs, in call order.
    let mut pending: Option<(usize, Vec<&str>)> = None;
    for (position, record) in messages.iter().enumerate() {
        let index = position + 1;
        if record.role != MessageRole::Assistant && !record.tool_calls.is_empty() {
            return Err(Error::Lua(format!(
                "messages[{index}] carries tool_calls but is not an assistant message"
            )));
        }
        if record.role != MessageRole::Tool && record.tool_call_id.is_some() {
            return Err(Error::Lua(format!(
                "messages[{index}] carries a tool_call_id but is not a tool message"
            )));
        }
        match record.role {
            MessageRole::System if position >= system_len => {
                return Err(Error::Lua(format!(
                    "messages[{index}] is a system message outside the leading system block"
                )));
            }
            MessageRole::Assistant if !record.tool_calls.is_empty() => {
                reject_unanswered(pending.as_ref())?;
                for call in &record.tool_calls {
                    if !seen.insert(call.id.as_str()) {
                        return Err(Error::Lua(format!(
                            "messages[{index}] tool call id {:?} duplicates an earlier \
                             tool call",
                            call.id
                        )));
                    }
                }
                pending = Some((
                    index,
                    record
                        .tool_calls
                        .iter()
                        .map(|call| call.id.as_str())
                        .collect(),
                ));
            }
            MessageRole::Tool => {
                // The protocol parse requires a string tool_call_id on every
                // tool record, so the option is always some here.
                let id = record.tool_call_id.as_deref().unwrap_or_default();
                match &mut pending {
                    Some((_, unanswered)) if unanswered.contains(&id) => {
                        unanswered.retain(|open| *open != id);
                        if unanswered.is_empty() {
                            pending = None;
                        }
                    }
                    _ => {
                        return Err(Error::Lua(format!(
                            "messages[{index}] is an orphan tool record: no pending \
                             assistant tool call with id {id:?}"
                        )));
                    }
                }
            }
            _ => reject_unanswered(pending.as_ref())?,
        }
    }
    reject_unanswered(pending.as_ref())
}

/// The incomplete-pairing failure for the open batch, if one is open: any
/// record (or the list's end) arriving before every call is answered breaks
/// the atomic call-result pairing.
fn reject_unanswered(pending: Option<&(usize, Vec<&str>)>) -> Result<()> {
    if let Some((index, unanswered)) = pending
        && let Some(id) = unanswered.first()
    {
        return Err(Error::Lua(format!(
            "messages[{index}] tool call {id:?} has no tool result"
        )));
    }
    Ok(())
}

/// How many records open the list with the system role.
fn leading_system_len(messages: &[MessageRecord]) -> usize {
    messages
        .iter()
        .take_while(|record| record.role == MessageRole::System)
        .count()
}

/// The projection pass over a validated list: compose the leading system
/// block, heal same-role runs, and emit one wire message per projected
/// record. Total by construction - [`validate`] rejected every failure
/// shape, and the source array is never mutated.
fn project(messages: &[MessageRecord]) -> Vec<Message> {
    let system_len = leading_system_len(messages);
    let mut projected: Vec<MessageRecord> = Vec::with_capacity(messages.len());
    match system_len {
        0 => {}
        1 => projected.push(messages[0].clone()),
        // Composition is textual; validation refused parts in a composed
        // block, so the visible text is the whole content.
        _ => projected.push(MessageRecord {
            role: MessageRole::System,
            content: MessageContent::Text(
                messages[..system_len]
                    .iter()
                    .map(|record| visible_text(&record.content))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }),
    }
    for record in &messages[system_len..] {
        match projected.last_mut() {
            // Two utterances of one role join with a blank line; fragments
            // of one assistant reply reassemble with none.
            Some(last)
                if last.role == record.role
                    && last.tool_calls.is_empty()
                    && record.tool_calls.is_empty()
                    && matches!(last.role, MessageRole::User | MessageRole::Assistant) =>
            {
                let separator = match last.role {
                    MessageRole::User => "\n\n",
                    _ => "",
                };
                last.content = merge_content(&last.content, &record.content, separator);
            }
            // Visible text rides the tool-call turn it precedes rather than
            // breaking provider alternation as a lone assistant message.
            Some(last)
                if last.role == MessageRole::Assistant
                    && last.tool_calls.is_empty()
                    && record.role == MessageRole::Assistant =>
            {
                last.content = merge_content(&last.content, &record.content, "");
                last.tool_calls.clone_from(&record.tool_calls);
            }
            _ => projected.push(record.clone()),
        }
    }
    projected.iter().map(wire_message).collect()
}

/// Merges two contents under one message: plain texts join with
/// `separator`; when either side is a parts array the parts concatenate in
/// arrival order with the separator as a text part between them. An empty
/// text side contributes nothing, so stray empty fragments vanish into
/// their run instead of poisoning the join.
fn merge_content(left: &MessageContent, right: &MessageContent, separator: &str) -> MessageContent {
    if let (MessageContent::Text(a), MessageContent::Text(b)) = (left, right) {
        if a.is_empty() {
            MessageContent::Text(b.clone())
        } else if b.is_empty() {
            MessageContent::Text(a.clone())
        } else {
            MessageContent::Text(format!("{a}{separator}{b}"))
        }
    } else {
        let mut parts = content_parts(left);
        if !separator.is_empty() {
            parts.push(ContentPart::Text(separator.to_owned()));
        }
        parts.extend(content_parts(right));
        MessageContent::Parts(parts)
    }
}

/// One content as a parts vector: a plain text is its one text part.
fn content_parts(content: &MessageContent) -> Vec<ContentPart> {
    match content {
        MessageContent::Text(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![ContentPart::Text(text.clone())]
            }
        }
        MessageContent::Parts(parts) => parts.clone(),
    }
}

/// The visible text of one content: the text, or the concatenation of its
/// text parts. Only called on plain-text system records (validation refused
/// parts in a composed block), so this never drops an image.
fn visible_text(content: &MessageContent) -> &str {
    match content {
        MessageContent::Text(text) => text,
        MessageContent::Parts(_) => unreachable!("validation refused parts"),
    }
}

/// Converts one validated, projected record into its wire message: exactly
/// the four contract fields, with each tool call rendered as the
/// provider-neutral `{id, name, arguments}` object. This is the metadata
/// strip - nothing else a record ever carried can reach the provider.
fn wire_message(record: &MessageRecord) -> Message {
    let content = match &record.content {
        MessageContent::Text(text) => Value::String(text.clone()),
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text(text) => {
                    serde_json::json!({ "type": "text", "text": text })
                }
                ContentPart::ImageUrl(url) => {
                    serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": url },
                    })
                }
            })
            .collect(),
    };
    let tool_calls = if record.tool_calls.is_empty() {
        None
    } else {
        Some(
            record
                .tool_calls
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "id": call.id,
                        "name": call.name,
                        "arguments": call.arguments,
                    })
                })
                .collect(),
        )
    };
    Message::from_validated_parts(
        record.role.as_str(),
        content,
        record.tool_call_id.clone(),
        tool_calls,
    )
}

#[cfg(test)]
mod tests;
