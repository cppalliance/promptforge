use super::Progress;

#[test]
fn a_busy_snapshot_round_trips_through_json() {
    let wire = r#"{"busy":true,"text":"Downloading qwen3-8b.gguf 45%"}"#;
    let snapshot: Progress = serde_json::from_str(wire).expect("the wire shape must parse");
    assert_eq!(
        snapshot,
        Progress {
            busy: true,
            text: "Downloading qwen3-8b.gguf 45%".to_owned(),
        }
    );
    let again = serde_json::to_string(&snapshot).expect("a snapshot must serialize");
    assert_eq!(again, wire, "the round trip must be byte-for-byte lossless");
}

#[test]
fn the_default_snapshot_is_idle_with_empty_text() {
    let snapshot = Progress::default();
    assert!(!snapshot.busy, "an idle snapshot is not busy");
    assert_eq!(snapshot.text, "", "an idle snapshot has empty text");
    let wire = serde_json::to_string(&snapshot).expect("the default must serialize");
    assert_eq!(wire, r#"{"busy":false,"text":""}"#);
}
