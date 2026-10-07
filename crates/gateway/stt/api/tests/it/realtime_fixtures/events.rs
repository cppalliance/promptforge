//! Characterization of the canonical client, session, and server event fixtures.

use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

use super::{
    ErrorCorrelation, assert_error, assert_exact_cases, assert_exact_keys,
    assert_server_event_fields, assert_session, assert_wire_event, fixture, fixture_dir, object,
};

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
    "session_update_ranges",
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
    "transcription_hypothesis_ranges",
];

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

    let ranges_update = object(
        &clients["session_update_ranges"]["session"],
        "session_update_ranges.session",
    );
    assert_exact_keys(
        ranges_update,
        &["include", "type"],
        "session_update_ranges.session",
    );

    let sessions = fixture("effective-sessions.json");
    assert_exact_cases(
        &sessions,
        &["default", "ranges", "updated"],
        "effective sessions",
    );
    assert_session(&sessions["default"], "default session");
    assert_session(&sessions["updated"], "updated session");
    assert_session(&sessions["ranges"], "ranges session");

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

    let mut extended = object(&servers["transcription_hypothesis_ranges"], "ranges").clone();
    let mut base = hypothesis.clone();
    for field in ["event_id", "finalized_through_ms", "finalized_seq"] {
        extended.remove(field);
        base.remove(field);
    }
    assert_eq!(
        extended, base,
        "the ranges hypothesis is the base hypothesis plus the two range fields"
    );

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
