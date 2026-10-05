//! Tests for the wire bodies' trust-boundary validation and serde round-trips.

use gateway_api_types::{Capabilities, ModelKind, ThinkingMode};

use super::*;

mod chat;
mod embedding;
mod rerank;
mod speech;

#[test]
fn model_info_serializes_kind_in_catalog_spelling() {
    let info = |kind: ModelKind| ModelInfo {
        id: "m".to_owned(),
        object: "model",
        kind,
        description: "d".to_owned(),
        context: 8192,
        thinking: ThinkingMode::Never,
        capabilities: Capabilities::default(),
    };
    for (kind, spelling) in [
        (ModelKind::Chat, "chat"),
        (ModelKind::Embedding, "embedding"),
        (ModelKind::Classifier, "classifier"),
    ] {
        let json = serde_json::to_value(info(kind)).expect("serialize");
        assert_eq!(json.get("kind").and_then(Value::as_str), Some(spelling));
    }
}
