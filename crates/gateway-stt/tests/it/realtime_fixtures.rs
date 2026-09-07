#![expect(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "fixture characterization fails with the contract invariant named"
)]

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use serde_json::{Map, Value};

const FIXTURE_FILES: &[&str] = &[
    "client-events.json",
    "effective-sessions.json",
    "invalid-sequences.json",
    "server-events.json",
    "valid-sequences.json",
];

const CLIENT_CASES: &[&str] = &[
    "input_audio_buffer_append",
    "input_audio_buffer_clear",
    "input_audio_buffer_commit",
    "session_update",
];

const SERVER_CASES: &[&str] = &[
    "conversation_item_created",
    "error_correlated",
    "error_minimal",
    "error_uncorrelated",
    "input_audio_buffer_cleared",
    "input_audio_buffer_committed",
    "session_created",
    "session_updated",
    "transcription_completed",
    "transcription_delta",
    "transcription_failed",
    "transcription_hypothesis",
];

const VALID_SEQUENCE_CASES: &[&str] = &[
    "clear_retires_only_uncommitted_input",
    "configuration_snapshot_isolation",
    "durable_lineage",
    "engine_replacement",
    "first_event_readiness",
    "hypothesis_negotiation",
    "immediate_commit_and_provisional_promotion",
    "optional_client_ids_and_error_correlation",
    "overlapping_items_reverse_completion",
    "pending_precommit_failure_clear",
    "pending_precommit_failure_commit",
    "producer_hypothesis_ownership",
    "saturated_commit_retry",
    "segment_admission_failure",
    "standard_delta_after_item_creation",
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

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("realtime")
}

fn fixture(name: &str) -> Value {
    let bytes = std::fs::read(fixture_dir().join(name)).expect("canonical fixture reads");
    let parsed: Value = serde_json::from_slice(&bytes).expect("canonical fixture is JSON");
    let reparsed: Value =
        serde_json::from_str(&serde_json::to_string(&parsed).expect("fixture serializes"))
            .expect("serialized fixture parses");
    assert_eq!(
        reparsed, parsed,
        "{name} round-trips without semantic drift"
    );
    parsed
}

fn object<'a>(value: &'a Value, context: &str) -> &'a Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is an object"))
}

fn assert_exact_keys(object: &Map<String, Value>, expected: &[&str], context: &str) {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{context} has the strict field set");
}

fn assert_exact_cases(value: &Value, expected: &[&str], context: &str) {
    assert_exact_keys(object(value, context), expected, context);
}

fn assert_nonempty_string(value: Option<&Value>, context: &str) {
    assert!(
        value
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty()),
        "{context} is a nonempty string"
    );
}

fn assert_session(value: &Value, context: &str) {
    let session = object(value, context);
    assert_exact_keys(
        session,
        &["audio", "id", "include", "object", "type"],
        context,
    );
    assert_nonempty_string(session.get("id"), &format!("{context}.id"));
    assert_eq!(
        session.get("object").and_then(Value::as_str),
        Some("realtime.transcription_session")
    );
    assert_eq!(
        session.get("type").and_then(Value::as_str),
        Some("transcription")
    );
    let include = session
        .get("include")
        .and_then(Value::as_array)
        .expect("effective include is an array");
    assert!(
        include.len() <= 1,
        "effective include has at most one value"
    );
    if let Some(value) = include.first() {
        assert_eq!(
            value.as_str(),
            Some("item.input_audio_transcription.hypothesis")
        );
    }

    let audio = object(
        session.get("audio").expect("session has audio"),
        "session.audio",
    );
    assert_exact_keys(audio, &["input"], "session.audio");
    let input = object(
        audio.get("input").expect("session has audio input"),
        "session.audio.input",
    );
    assert_exact_keys(
        input,
        &[
            "format",
            "noise_reduction",
            "transcription",
            "turn_detection",
        ],
        "session.audio.input",
    );
    assert!(input["noise_reduction"].is_null());
    assert!(input["turn_detection"].is_null());

    let format = object(&input["format"], "session.audio.input.format");
    assert_exact_keys(format, &["rate", "type"], "session.audio.input.format");
    assert_eq!(format["type"], "audio/pcm");
    assert_eq!(format["rate"], 24_000);

    let transcription = object(&input["transcription"], "session.audio.input.transcription");
    assert_exact_keys(
        transcription,
        &["model", "prompt"],
        "session.audio.input.transcription",
    );
    assert_eq!(transcription["model"], "realtime-transcribe");
    assert!(transcription["prompt"].is_string());
}

#[derive(Clone, Copy)]
enum ErrorCorrelation<'a> {
    Omitted,
    Null,
    Client(&'a str),
}

fn assert_error(value: &Value, correlation: ErrorCorrelation<'_>, context: &str) {
    let event = object(value, context);
    assert_exact_keys(event, &["error", "event_id", "type"], context);
    assert_nonempty_string(event.get("event_id"), &format!("{context}.event_id"));
    assert_eq!(event["type"], "error");
    let error = object(&event["error"], &format!("{context}.error"));
    let required = ["code", "message", "type"];
    assert!(
        required.iter().all(|field| error.contains_key(*field)),
        "{context}.error has every required field"
    );
    assert!(
        error.keys().all(|field| matches!(
            field.as_str(),
            "code" | "event_id" | "message" | "param" | "type"
        )),
        "{context}.error has no unsupported field"
    );
    for field in ["type", "code", "message"] {
        assert_nonempty_string(error.get(field), &format!("{context}.error.{field}"));
    }
    if let Some(param) = error.get("param") {
        assert!(param.is_null() || param.is_string());
    }
    match correlation {
        ErrorCorrelation::Omitted => assert!(!error.contains_key("event_id")),
        ErrorCorrelation::Null => assert!(error["event_id"].is_null()),
        ErrorCorrelation::Client(expected) => assert_eq!(error["event_id"], expected),
    }
}

fn assert_wire_event(value: &Value, direction: &str, context: &str) {
    let event = object(value, context);
    assert_nonempty_string(event.get("type"), &format!("{context}.type"));
    match direction {
        "client" => {
            if let Some(event_id) = event.get("event_id") {
                assert_nonempty_string(Some(event_id), &format!("{context}.event_id"));
            }
        }
        "server" => assert_nonempty_string(event.get("event_id"), &format!("{context}.event_id")),
        other => panic!("{context} has unsupported direction {other}"),
    }
}

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

fn assert_server_event_fields(value: &Value, context: &str) {
    let event = object(value, context);
    let event_type = event["type"]
        .as_str()
        .expect("server event type is a string");
    let fields = match event_type {
        "session.created" | "session.updated" => &["event_id", "session", "type"][..],
        "input_audio_buffer.committed" => &["event_id", "item_id", "previous_item_id", "type"][..],
        "input_audio_buffer.cleared" => &["event_id", "type"][..],
        "conversation.item.created" => &["event_id", "item", "previous_item_id", "type"][..],
        "conversation.item.input_audio_transcription.delta" => {
            &["content_index", "delta", "event_id", "item_id", "type"][..]
        }
        "conversation.item.input_audio_transcription.completed" => &[
            "content_index",
            "event_id",
            "item_id",
            "transcript",
            "type",
            "usage",
        ][..],
        "conversation.item.input_audio_transcription.failed" => {
            &["content_index", "error", "event_id", "item_id", "type"][..]
        }
        "conversation.item.input_audio_transcription.hypothesis" => &[
            "agreed",
            "audio_end_ms",
            "audio_start_ms",
            "content_index",
            "event_id",
            "finalized",
            "item_id",
            "revision",
            "tentative",
            "transcript",
            "type",
        ][..],
        "error" => &["error", "event_id", "type"][..],
        other => panic!("{context} has unsupported server event type {other}"),
    };
    assert_exact_keys(event, fields, context);
    if matches!(event_type, "session.created" | "session.updated") {
        assert_session(&event["session"], &format!("{context}.session"));
    }
    if let Some(content_index) = event.get("content_index") {
        assert_eq!(content_index, 0, "{context}.content_index is zero");
    }
    if let Some(previous) = event.get("previous_item_id") {
        assert!(
            previous.is_null() || previous.as_str().is_some_and(|id| !id.is_empty()),
            "{context}.previous_item_id is null or opaque"
        );
    }
    if event_type == "conversation.item.created" {
        let item = object(&event["item"], &format!("{context}.item"));
        assert_exact_keys(
            item,
            &["content", "id", "role", "status", "type"],
            &format!("{context}.item"),
        );
        assert_nonempty_string(item.get("id"), &format!("{context}.item.id"));
        assert_eq!(item["type"], "message");
        assert_eq!(item["status"], "completed");
        assert_eq!(item["role"], "user");
        let item_entries = item["content"]
            .as_array()
            .filter(|entries| entries.len() == 1)
            .expect("created item has exactly one content entry");
        let audio_entry = object(&item_entries[0], &format!("{context}.item.content[0]"));
        assert_exact_keys(
            audio_entry,
            &["transcript", "type"],
            &format!("{context}.item.content[0]"),
        );
        assert_eq!(audio_entry["type"], "input_audio");
        assert!(audio_entry["transcript"].is_null());
    }
    if event_type == "conversation.item.input_audio_transcription.failed" {
        let error = object(&event["error"], &format!("{context}.error"));
        assert!(
            ["code", "message", "type"]
                .iter()
                .all(|field| error.contains_key(*field)),
            "{context}.error has every required field"
        );
        assert!(
            error
                .keys()
                .all(|field| matches!(field.as_str(), "code" | "message" | "param" | "type"))
        );
    }
}

#[test]
fn canonical_realtime_events_are_complete_strict_and_round_trip() {
    let actual_files = std::fs::read_dir(fixture_dir())
        .expect("canonical fixture directory reads")
        .map(|entry| {
            entry
                .expect("fixture directory entry reads")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    let expected_files = FIXTURE_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_files, expected_files,
        "fixture file set is canonical"
    );

    let clients = fixture("client-events.json");
    assert_exact_cases(&clients, CLIENT_CASES, "client event cases");
    let clients = object(&clients, "client event cases");
    let client_types = [
        ("session_update", "session.update"),
        ("input_audio_buffer_append", "input_audio_buffer.append"),
        ("input_audio_buffer_commit", "input_audio_buffer.commit"),
        ("input_audio_buffer_clear", "input_audio_buffer.clear"),
    ];
    for (case, event_type) in client_types {
        let event = object(&clients[case], case);
        assert_eq!(event["type"], event_type, "{case} pins its type literal");
        assert_wire_event(&clients[case], "client", case);
    }
    assert_exact_keys(
        object(&clients["session_update"], "session_update"),
        &["event_id", "session", "type"],
        "session_update",
    );
    assert_exact_keys(
        object(&clients["input_audio_buffer_append"], "append"),
        &["audio", "event_id", "type"],
        "append",
    );
    assert_exact_keys(
        object(&clients["input_audio_buffer_commit"], "commit"),
        &["type"],
        "commit",
    );
    assert_exact_keys(
        object(&clients["input_audio_buffer_clear"], "clear"),
        &["type"],
        "clear",
    );
    let update = object(
        &clients["session_update"]["session"],
        "session_update.session",
    );
    assert_exact_keys(
        update,
        &["audio", "include", "type"],
        "session_update.session",
    );
    assert_eq!(update["type"], "transcription");
    let update_audio = object(&update["audio"], "session_update.session.audio");
    assert_exact_keys(update_audio, &["input"], "session_update.session.audio");
    let update_input = object(&update_audio["input"], "session_update.session.audio.input");
    assert_exact_keys(
        update_input,
        &[
            "format",
            "noise_reduction",
            "transcription",
            "turn_detection",
        ],
        "session_update.session.audio.input",
    );

    let sessions = fixture("effective-sessions.json");
    assert_exact_cases(&sessions, &["default", "updated"], "effective sessions");
    assert_session(&sessions["default"], "default session");
    assert_session(&sessions["updated"], "updated session");

    let servers = fixture("server-events.json");
    assert_exact_cases(&servers, SERVER_CASES, "server event cases");
    let servers = object(&servers, "server event cases");
    for (case, event) in servers {
        assert_wire_event(event, "server", case);
        assert_server_event_fields(event, case);
    }
    assert_eq!(servers["session_created"]["session"], sessions["default"]);
    assert_eq!(servers["session_updated"]["session"], sessions["updated"]);
    assert_error(
        &servers["error_correlated"],
        ErrorCorrelation::Client("client_bad_update"),
        "error_correlated",
    );
    assert_error(
        &servers["error_minimal"],
        ErrorCorrelation::Omitted,
        "error_minimal",
    );
    assert_error(
        &servers["error_uncorrelated"],
        ErrorCorrelation::Null,
        "error_uncorrelated",
    );

    let completed = object(&servers["transcription_completed"], "completed");
    let usage = object(&completed["usage"], "completed.usage");
    assert_exact_keys(usage, &["seconds", "type"], "completed.usage");
    assert_eq!(usage["type"], "duration");
    assert!(
        usage["seconds"]
            .as_f64()
            .is_some_and(|seconds| seconds >= 0.0),
        "duration usage is nonnegative"
    );

    let hypothesis = object(&servers["transcription_hypothesis"], "hypothesis");
    let joined = ["finalized", "agreed", "tentative"]
        .map(|field| {
            hypothesis[field]
                .as_str()
                .expect("hypothesis text is a string")
        })
        .concat();
    assert_eq!(hypothesis["transcript"], joined);
    assert!(hypothesis["revision"].as_u64().is_some());
    let start = hypothesis["audio_start_ms"]
        .as_u64()
        .expect("hypothesis start is unsigned");
    let end = hypothesis["audio_end_ms"]
        .as_u64()
        .expect("hypothesis end is unsigned");
    assert!(start <= end, "hypothesis span is half-open and ordered");

    let mut server_ids = HashSet::new();
    for event in servers.values() {
        let id = event["event_id"]
            .as_str()
            .expect("server event ID is a string");
        assert!(server_ids.insert(id), "server event IDs are independent");
    }
    let session_ids = sessions
        .as_object()
        .expect("sessions object")
        .values()
        .map(|session| session["id"].as_str().expect("session ID is a string"))
        .collect::<HashSet<_>>();
    let item_ids = servers
        .values()
        .filter_map(|event| event.get("item_id").and_then(Value::as_str))
        .chain(servers["conversation_item_created"]["item"]["id"].as_str())
        .collect::<HashSet<_>>();
    assert!(server_ids.is_disjoint(&session_ids));
    assert!(server_ids.is_disjoint(&item_ids));
    assert!(session_ids.is_disjoint(&item_ids));
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
