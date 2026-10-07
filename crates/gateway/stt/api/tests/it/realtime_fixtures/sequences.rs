//! Characterization of the valid and invalid event sequence fixtures.

use serde_json::Value;

use super::{
    ErrorCorrelation, assert_error, assert_exact_cases, assert_exact_keys,
    assert_server_event_fields, assert_wire_event, fixture, object,
};

const VALID_SEQUENCE_CASES: &[&str] = &[
    "clear_retires_only_uncommitted_input",
    "configuration_snapshot_isolation",
    "durable_lineage",
    "first_event_readiness",
    "hypothesis_negotiation",
    "hypothesis_ranges_negotiation",
    "immediate_commit_and_provisional_promotion",
    "optional_client_ids_and_error_correlation",
    "overlapping_items_reverse_completion",
    "pending_precommit_failure_clear",
    "pending_precommit_failure_commit",
    "producer_hypothesis_ownership",
    "saturated_commit_retry",
    "standard_live_deltas_before_commit",
];

const INVALID_SEQUENCE_CASES: &[&str] = &[
    "append_after_precommit_failure",
    "append_invalid_base64",
    "append_limit_exceeded",
    "append_unknown_field",
    "clear_unknown_field",
    "commit_short_audio",
    "commit_unknown_field",
    "dangling_pcm_byte_on_commit",
    "excessive_queue_lag",
    "invalid_client_event_id",
    "invalid_include_type",
    "invalid_prompt_type",
    "malformed_json",
    "maximum_committed_items",
    "maximum_unfinalized_audio",
    "missing_append_audio",
    "missing_client_event_type",
    "missing_session",
    "missing_session_type",
    "non_null_noise_reduction",
    "non_null_turn_detection",
    "ranges_include_without_hypothesis",
    "repeated_include",
    "result_queue_overload",
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

const MINIMUM_COMMIT_AUDIO_BYTES: usize = 24_000 * 2 / 10;

fn canonical_base64_decoded_len(value: &str, context: &str) -> usize {
    assert!(value.is_ascii(), "{context} Base64 is ASCII");
    assert_eq!(value.len() % 4, 0, "{context} Base64 has complete quartets");
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    assert!(padding <= 2, "{context} Base64 has valid padding");
    let payload_len = value.len() - padding;
    assert!(
        value[..payload_len]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/')),
        "{context} Base64 has only alphabet characters"
    );
    assert!(
        value[payload_len..].bytes().all(|byte| byte == b'='),
        "{context} Base64 padding is trailing"
    );
    value.len() / 4 * 3 - padding
}

fn assert_valid_commit_audio(name: &str, events: &[Value]) {
    let mut buffered_audio_bytes = 0;
    for (index, entry) in events.iter().enumerate() {
        let direction = entry["direction"].as_str().expect("direction is a string");
        let message = object(&entry["message"], &format!("{name}[{index}].message"));
        let event_type = message["type"].as_str().expect("event type is a string");
        if direction == "client" {
            match event_type {
                "input_audio_buffer.append" => {
                    let audio = message["audio"].as_str().expect("append audio is a string");
                    buffered_audio_bytes += canonical_base64_decoded_len(
                        audio,
                        &format!("{name}[{index}].message.audio"),
                    );
                }
                "input_audio_buffer.commit" => assert!(
                    buffered_audio_bytes >= MINIMUM_COMMIT_AUDIO_BYTES,
                    "{name}[{index}] commits {buffered_audio_bytes} PCM16 bytes, below 100 ms"
                ),
                _ => {}
            }
        } else if matches!(
            event_type,
            "input_audio_buffer.committed" | "input_audio_buffer.cleared"
        ) {
            buffered_audio_bytes = 0;
        }
    }
}

#[test]
fn canonical_realtime_sequences_cover_valid_and_invalid_contract_paths() {
    let valid = fixture("valid-sequences.json");
    assert_exact_cases(&valid, VALID_SEQUENCE_CASES, "valid sequence cases");
    for (name, sequence) in object(&valid, "valid sequence cases") {
        let sequence = object(sequence, name);
        assert_exact_keys(sequence, &["events", "invariants"], name);
        let events = sequence["events"]
            .as_array()
            .expect("valid sequence events are an array");
        assert!(!events.is_empty(), "{name} has events");
        for (index, entry) in events.iter().enumerate() {
            let entry = object(entry, &format!("{name}[{index}]"));
            assert_exact_keys(
                entry,
                &["direction", "message"],
                &format!("{name}[{index}]"),
            );
            let direction = entry["direction"]
                .as_str()
                .expect("sequence direction is a string");
            assert_wire_event(
                &entry["message"],
                direction,
                &format!("{name}[{index}].message"),
            );
            if direction == "server" {
                assert_server_event_fields(&entry["message"], &format!("{name}[{index}].message"));
            }
        }
        assert_valid_commit_audio(name, events);
        let invariants = sequence["invariants"]
            .as_array()
            .expect("valid sequence invariants are an array");
        assert!(
            !invariants.is_empty() && invariants.iter().all(Value::is_string),
            "{name} names the behavior it freezes"
        );
    }
    let revisions = valid["hypothesis_negotiation"]["events"]
        .as_array()
        .expect("hypothesis sequence events")
        .iter()
        .filter(|entry| {
            entry["message"]["type"] == "conversation.item.input_audio_transcription.hypothesis"
        })
        .map(|entry| {
            entry["message"]["revision"]
                .as_u64()
                .expect("hypothesis revision is unsigned")
        })
        .collect::<Vec<_>>();
    assert_eq!(revisions, [1, 2], "hypothesis revisions increase");

    let standard = valid["standard_live_deltas_before_commit"]["events"]
        .as_array()
        .expect("standard delta sequence events")
        .iter()
        .filter(|entry| entry["direction"] == "server")
        .map(|entry| &entry["message"])
        .collect::<Vec<_>>();
    let position = |event_type: &str| {
        standard
            .iter()
            .position(|message| message["type"] == event_type)
            .unwrap_or_else(|| panic!("standard delta sequence has {event_type}"))
    };
    assert!(
        position("conversation.item.input_audio_transcription.delta")
            < position("input_audio_buffer.committed"),
        "a standard delta arrives before commit"
    );
    let streamed = standard
        .iter()
        .filter(|message| message["type"] == "conversation.item.input_audio_transcription.delta")
        .map(|message| message["delta"].as_str().expect("delta is text"))
        .collect::<String>();
    assert_eq!(
        streamed, "Hello big world",
        "standard deltas append to one another"
    );

    let invalid = fixture("invalid-sequences.json");
    assert_exact_cases(&invalid, INVALID_SEQUENCE_CASES, "invalid sequence cases");
    for (name, sequence) in object(&invalid, "invalid sequence cases") {
        let sequence = object(sequence, name);
        assert_exact_keys(
            sequence,
            &[
                "effective_session_after",
                "expected_error",
                "input",
                "keeps_connection_usable",
            ],
            name,
        );
        assert!(
            sequence["keeps_connection_usable"]
                .as_bool()
                .is_some_and(|usable| usable),
            "{name} is a recoverable client or capacity error"
        );
        assert!(
            matches!(
                sequence["effective_session_after"].as_str(),
                Some("default" | "updated")
            ),
            "{name} names the unchanged effective session"
        );
        let input = object(&sequence["input"], &format!("{name}.input"));
        let correlation = match input.get("message") {
            None => ErrorCorrelation::Omitted,
            Some(message) => {
                match object(message, &format!("{name}.input.message")).get("event_id") {
                    None => ErrorCorrelation::Omitted,
                    Some(Value::String(client_id)) => ErrorCorrelation::Client(client_id),
                    Some(_) => ErrorCorrelation::Null,
                }
            }
        };
        assert_error(
            &sequence["expected_error"],
            correlation,
            &format!("{name}.expected_error"),
        );
        assert!(
            input.contains_key("message") || input.contains_key("wire_text"),
            "{name} supplies a wire message or malformed wire text"
        );
    }
}
