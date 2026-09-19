//! The effect record: every effect kind projects onto a record that
//! round-trips through serde, and the projection drops exactly the live
//! handles.

use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge_api_types::tools::ToolId;
use promptforge_model_client::model::{ModelInvocation, Temperature};
use serde_json::json;

use super::*;
use crate::model::ModelId;

/// Serializes the record and reads it back: the round trip a run log and
/// a replay depend on.
fn round_trip(record: &EffectRecord) -> EffectRecord {
    let text = serde_json::to_string(record).expect("a record serializes");
    serde_json::from_str(&text).expect("a serialized record deserializes")
}

fn binding() -> ModelBinding {
    ModelBinding::new(
        "writer",
        "A general model for tests",
        ModelId::from_validated("gateway", "test-model"),
        ModelInvocation {
            temperature: Some(Temperature::new(0.2).expect("0.2 is in range")),
            max_tokens: NonZeroU32::new(256),
            thinking: Some(false),
        },
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

#[test]
fn a_chat_effect_records_its_model_messages_tools_and_invocation() {
    let binding = binding();
    let effect = Effect::Chat {
        options: binding.completion_options(),
        binding,
        messages: vec![Message::user("ask")],
        tools: vec![
            ToolSchema::new("grab", "Grab a value", json!({ "type": "object" }))
                .expect("a valid schema"),
        ],
        stream: true,
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::Chat {
            model: "test-model".to_owned(),
            alias: "writer".to_owned(),
            messages: vec![json!({ "role": "user", "content": "ask" })],
            tools: vec!["grab".to_owned()],
            temperature: Some(0.2),
            max_tokens: Some(256),
            thinking: Some(false),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_tool_call_effect_records_its_identity_alias_and_args() {
    let effect = Effect::ToolCall {
        tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
        alias: "echo".to_owned(),
        args: json!({ "value": "hi" }),
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::ToolCall {
            tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_user_input_effect_records_its_execution_and_section() {
    let effect = Effect::UserInput {
        execution: "run-1".to_owned(),
        section: "Only".to_owned(),
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::UserInput {
            execution: "run-1".to_owned(),
            section: "Only".to_owned(),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_store_effect_records_its_operation_and_drops_the_access_handle() {
    let access = Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("run test fixture"))
            .expect("the stock backend acquires"),
    );
    let effect = Effect::Store {
        access,
        op: StoreOp::Read {
            path: "notes.md".to_owned(),
            start: Some(1),
            end: None,
        },
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::Store {
            op: StoreOp::Read {
                path: "notes.md".to_owned(),
                start: Some(1),
                end: None,
            },
        }
    );
    // The record is the operation alone: nothing of the handle survives.
    let text = serde_json::to_string(&record).expect("a record serializes");
    assert!(
        !text.contains("access"),
        "the store record carries no handle: {text}"
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_timer_effect_records_its_seconds() {
    let effect = Effect::Timer { seconds: 0.25 };
    let record = effect.record();
    assert_eq!(record, EffectRecord::Timer { seconds: 0.25 });
    assert_eq!(round_trip(&record), record);
}
