//! Characterization tests for the realtime protocol JSON fixture files.

#![expect(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "fixture characterization fails with the contract invariant named"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Map, Value};

mod events;
mod sequences;

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
