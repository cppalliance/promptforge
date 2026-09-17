//! Catalog frames: the `{"type":"models",...}` pushes.

use serde::Serialize;

/// One pushed model catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogPush {
    /// The chat-capable subset of the gateway's model array.
    pub models: Vec<serde_json::Value>,
}

impl CatalogPush {
    /// The push as a wire frame: `"type": "models"` beside the array.
    #[must_use]
    pub fn frame(&self) -> CatalogFrame<'_> {
        CatalogFrame {
            kind: "models",
            models: &self.models,
        }
    }
}

/// The serialized shape of a catalog push on the socket, matching the
/// workshop protocol's frame taxonomy.
///
/// Delivery: ephemeral - the newest push carries the whole catalog and
/// supersedes every older one; the catalog is resent on reconnect.
#[derive(Debug, Serialize)]
pub struct CatalogFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    models: &'a [serde_json::Value],
}

/// Whether one gateway catalog row can back a chat model binding.
///
/// This is wire semantics - it interprets the gateway's catalog shape -
/// so it lives here rather than in any one subsystem: the menu filters
/// its picker by it, and the gateway subsystem's catalog refresh reads
/// readiness from it.
#[must_use]
pub fn is_chat_capable(model: &serde_json::Value) -> bool {
    let has_id = model
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let chat_kind = match model.get("kind") {
        None => true,
        Some(serde_json::Value::String(kind)) => kind == "chat",
        Some(_) => false,
    };
    has_id && chat_kind
}
