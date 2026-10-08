//! Fixture-driven tests for realtime wire parsing and serialization.

use std::collections::HashSet;

use serde_json::Value;

use super::{EffectiveSession, IdGenerator, ServerEvent, parse_client_event};
use crate::realtime::result_mailbox::{ItemFailure, ItemResult};

const WIRE_INVALID_CASES: &[&str] = &[
    "append_unknown_field",
    "clear_unknown_field",
    "commit_unknown_field",
    "invalid_client_event_id",
    "invalid_include_type",
    "invalid_prompt_type",
    "malformed_json",
    "missing_append_audio",
    "missing_client_event_type",
    "missing_session",
    "missing_session_type",
    "non_null_noise_reduction",
    "non_null_turn_detection",
    "ranges_include_without_hypothesis",
    "repeated_include",
    "session_audio_unknown_field",
    "session_input_unknown_field",
    "session_transcription_unknown_field",
    "session_unknown_field",
    "session_update_unknown_field",
    "unknown_event_type",
    "unknown_include",
    "unsupported_delay",
    "unsupported_format_rate",
    "unsupported_format_type",
    "unsupported_keywords",
    "unsupported_language",
    "unsupported_logprobs",
    "unsupported_model",
    "wrong_session_type",
];

fn fixture(name: &str) -> Value {
    let source = match name {
        "client-events.json" => {
            include_str!("../../../tests/fixtures/realtime/client-events.json")
        }
        "effective-sessions.json" => {
            include_str!("../../../tests/fixtures/realtime/effective-sessions.json")
        }
        "invalid-sequences.json" => {
            include_str!("../../../tests/fixtures/realtime/invalid-sequences.json")
        }
        "server-events.json" => {
            include_str!("../../../tests/fixtures/realtime/server-events.json")
        }
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(source).unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn canonical_client_events_parse_and_updates_are_atomic() {
    let clients = fixture("client-events.json");
    for event in clients
        .as_object()
        .unwrap_or_else(|| panic!("client fixture object"))
        .values()
    {
        let text = serde_json::to_string(event)
            .unwrap_or_else(|error| panic!("client fixture serializes: {error}"));
        parse_client_event(&text)
            .unwrap_or_else(|error| panic!("canonical event rejected: {error:?}"));
    }

    let sessions = fixture("effective-sessions.json");
    let mut effective = EffectiveSession::new("sess_canonical".to_owned());
    assert_eq!(
        serde_json::to_value(&effective)
            .unwrap_or_else(|error| panic!("default session serializes: {error}")),
        sessions["default"]
    );
    let update = serde_json::to_string(&clients["session_update"])
        .unwrap_or_else(|error| panic!("update fixture serializes: {error}"));
    effective
        .apply_update_text(&update)
        .unwrap_or_else(|error| panic!("canonical update applies: {error:?}"));
    assert_eq!(
        serde_json::to_value(&effective)
            .unwrap_or_else(|error| panic!("updated session serializes: {error}")),
        sessions["updated"]
    );

    let invalid = fixture("invalid-sequences.json");
    for case in WIRE_INVALID_CASES {
        let before = effective.clone();
        let input = &invalid[*case]["input"];
        let result = if let Some(text) = input["wire_text"].as_str() {
            effective.apply_update_text(text)
        } else {
            let text = serde_json::to_string(&input["message"])
                .unwrap_or_else(|error| panic!("{case} serializes: {error}"));
            effective.apply_update_text(&text)
        };
        let error = result.unwrap_err();
        let expected = &invalid[*case]["expected_error"];
        let event_id = expected["event_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{case} has server event ID"));
        assert_eq!(error.into_server_event(event_id), *expected, "{case}");
        assert_eq!(effective, before, "{case} must not partially update");
    }
}

#[test]
fn mixed_valid_and_invalid_session_updates_change_no_effective_state() {
    let clients = fixture("client-events.json");
    let update = serde_json::to_string(&clients["session_update"])
        .unwrap_or_else(|error| panic!("update fixture serializes: {error}"));
    let mut effective = EffectiveSession::new("sess_atomic".to_owned());
    effective
        .apply_update_text(&update)
        .unwrap_or_else(|error| panic!("canonical update applies: {error:?}"));
    let before = effective.clone();

    let valid_prompt_invalid_include = serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {"input": {"transcription": {"prompt": "must not apply"}}},
            "include": ["unsupported.include"]
        }
    });
    assert!(
        effective
            .apply_update_text(&valid_prompt_invalid_include.to_string())
            .is_err()
    );
    assert_eq!(
        effective, before,
        "valid prompt must not apply when include is invalid"
    );

    let valid_include_invalid_prompt = serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {"input": {"transcription": {"prompt": 7}}},
            "include": []
        }
    });
    assert!(
        effective
            .apply_update_text(&valid_include_invalid_prompt.to_string())
            .is_err()
    );
    assert_eq!(
        effective, before,
        "valid include must not apply when prompt is invalid"
    );
}

#[test]
fn ranges_include_extends_the_hypothesis_include_in_either_order() {
    let clients = fixture("client-events.json");
    let sessions = fixture("effective-sessions.json");
    let mut effective = EffectiveSession::new("sess_canonical".to_owned());
    effective
        .apply_update_text(&clients["session_update_ranges"].to_string())
        .unwrap_or_else(|error| panic!("ranges update applies: {error:?}"));
    assert_eq!(
        serde_json::to_value(&effective)
            .unwrap_or_else(|error| panic!("ranges session serializes: {error}")),
        sessions["ranges"]
    );

    let mut reversed = clients["session_update_ranges"].clone();
    reversed["session"]["include"] = serde_json::json!([
        "item.input_audio_transcription.hypothesis.ranges",
        "item.input_audio_transcription.hypothesis"
    ]);
    let mut reversed_effective = EffectiveSession::new("sess_canonical".to_owned());
    reversed_effective
        .apply_update_text(&reversed.to_string())
        .unwrap_or_else(|error| panic!("reversed ranges update applies: {error:?}"));
    assert_eq!(reversed_effective, effective);

    effective
        .apply_update_text(&clients["session_update"].to_string())
        .unwrap_or_else(|error| panic!("base update applies: {error:?}"));
    assert_eq!(
        serde_json::to_value(&effective)
            .unwrap_or_else(|error| panic!("updated session serializes: {error}")),
        sessions["updated"],
        "the base token alone drops the ranges extension"
    );
}

#[test]
fn hypothesis_range_fields_appear_together_and_are_never_null() {
    let servers = fixture("server-events.json");
    let ranges = &servers["transcription_hypothesis_ranges"];
    for field in ["finalized_through_ms", "finalized_seq"] {
        let mut lone = ranges.clone();
        lone.as_object_mut()
            .unwrap_or_else(|| panic!("ranges hypothesis is an object"))
            .remove(field);
        assert!(
            ServerEvent::from_value(lone).is_err(),
            "the other range field cannot appear without {field}"
        );
        let mut null = ranges.clone();
        null[field] = Value::Null;
        assert!(
            ServerEvent::from_value(null).is_err(),
            "{field} is a number when present"
        );
    }
}

#[test]
fn canonical_server_events_round_trip_with_exact_shapes() {
    let fixture = fixture("server-events.json");
    for (case, value) in fixture
        .as_object()
        .unwrap_or_else(|| panic!("server fixture object"))
    {
        let event = ServerEvent::from_value(value.clone())
            .unwrap_or_else(|error| panic!("{case} rejected: {error}"));
        assert_eq!(
            serde_json::to_value(event)
                .unwrap_or_else(|error| panic!("{case} serializes: {error}")),
            *value,
            "{case}"
        );
    }
}

#[test]
fn omission_of_each_required_nullable_server_field_is_rejected() {
    let servers = fixture("server-events.json");
    let cases = [
        (
            "session_created",
            &["session", "audio", "input", "noise_reduction"][..],
        ),
        (
            "session_updated",
            &["session", "audio", "input", "turn_detection"][..],
        ),
        ("input_audio_buffer_committed", &["previous_item_id"][..]),
        ("conversation_item_created", &["previous_item_id"][..]),
        (
            "conversation_item_created",
            &["item", "content", "0", "transcript"][..],
        ),
    ];
    for (name, path) in cases {
        let mut value = servers[name].clone();
        remove_path(&mut value, path);
        assert!(
            ServerEvent::from_value(value).is_err(),
            "{name} must reject omitted {}",
            path.join(".")
        );
    }
}

fn remove_path(value: &mut Value, path: &[&str]) {
    let (field, parents) = path
        .split_last()
        .unwrap_or_else(|| panic!("required field path is nonempty"));
    let mut parent = value;
    for segment in parents {
        parent = if let Ok(index) = segment.parse::<usize>() {
            &mut parent[index]
        } else {
            &mut parent[*segment]
        };
    }
    parent
        .as_object_mut()
        .unwrap_or_else(|| panic!("required field parent is an object"))
        .remove(*field)
        .unwrap_or_else(|| panic!("required field exists"));
}

#[test]
fn server_session_event_and_item_ids_use_independent_namespaces() {
    let ids = IdGenerator::default();
    let mut events = HashSet::new();
    let mut sessions = HashSet::new();
    let mut items = HashSet::new();
    for _ in 0..64 {
        assert!(events.insert(ids.event()));
        assert!(sessions.insert(ids.session()));
        assert!(items.insert(ids.item()));
    }
    assert!(events.is_disjoint(&sessions));
    assert!(events.is_disjoint(&items));
    assert!(sessions.is_disjoint(&items));
    assert!(!events.contains("client_event"));
    assert!(!sessions.contains("client_event"));
    assert!(!items.contains("client_event"));
}

#[test]
fn invalid_duration_usage_and_hypothesis_shapes_are_rejected() {
    let servers = fixture("server-events.json");
    let mut completed = servers["transcription_completed"].clone();
    completed["usage"]["seconds"] = Value::from(-0.01);
    assert!(ServerEvent::from_value(completed).is_err());

    let mut hypothesis = servers["transcription_hypothesis"].clone();
    hypothesis["transcript"] = Value::from("not the three parts");
    assert!(ServerEvent::from_value(hypothesis).is_err());
    let mut reversed_span = servers["transcription_hypothesis"].clone();
    reversed_span["audio_start_ms"] = Value::from(1251_u64);
    assert!(ServerEvent::from_value(reversed_span).is_err());

    let mut failed = servers["transcription_failed"].clone();
    failed["error"]["event_id"] = Value::Null;
    assert!(ServerEvent::from_value(failed).is_err());

    let empty_client_id = r#"{"type":"input_audio_buffer.clear","event_id":""}"#;
    let error = parse_client_event(empty_client_id).unwrap_err();
    assert_eq!(
        error.into_server_event("evt_empty_client_id"),
        serde_json::json!({
            "event_id": "evt_empty_client_id",
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "code": "invalid_event_id",
                "message": "event_id must be a string",
                "param": "event_id",
                "event_id": null
            }
        })
    );
}

#[test]
fn a_failed_authoritative_transcription_names_its_cause_and_a_precommit_failure_keeps_its_text() {
    let failed = |failure| {
        let event = ServerEvent::item_result(
            "evt_failed".to_owned(),
            ItemResult::Failed {
                item_id: "item_failed".to_owned(),
                failure,
            },
        );
        serde_json::to_value(event).unwrap_or_else(|error| panic!("failure serializes: {error}"))
    };

    let authoritative = failed(ItemFailure::TranscriptionFailed(
        "forced final window was not decodable".to_owned(),
    ));
    assert_eq!(authoritative["error"]["code"], "transcription_failed");
    assert_eq!(
        authoritative["error"]["message"],
        "Authoritative transcription failed: forced final window was not decodable"
    );

    let precommit = failed(ItemFailure::PrecommitTranscriptionFailed(
        "worker detail".to_owned(),
    ));
    assert_eq!(
        precommit["error"]["message"], "Accurate precommit transcription failed",
        "the precommit text stays fixed"
    );
}
