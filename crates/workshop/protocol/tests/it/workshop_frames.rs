//! The shared workshop-frame fixture pins: the same JSON the SPA suite
//! (`crates/workshop/ui/test/workshop-wire-fixtures.mjs`) asserts, so a
//! wire drift on either side fails that side's fixture test.

use workshop_protocol::{
    Activity, CatalogPush, ErrorFrame, SelectModelFrame, Severity, StatusBarUpdate,
    SwitchProfileFrame, WorkbenchSnapshot,
};

/// The shared workshop-frame fixture, asserted as the same JSON by the
/// SPA suite: a wire drift on either side fails that side's fixture test.
const WORKSHOP_FRAME_FIXTURE: &str = include_str!("../fixtures/workshop-frames.json");

/// Parses the shared fixture into one object keyed by case name.
fn workshop_fixture() -> serde_json::Value {
    match serde_json::from_str(WORKSHOP_FRAME_FIXTURE) {
        Ok(fixture) => fixture,
        Err(error) => panic!("the fixture is valid JSON: {error}"),
    }
}

#[test]
fn the_shared_fixture_pins_exactly_the_agreed_case_list() {
    let fixture = workshop_fixture();
    let mut cases: Vec<&str> = fixture
        .as_object()
        .expect("the fixture is one object keyed by case name")
        .keys()
        .map(String::as_str)
        .collect();
    cases.sort_unstable();
    assert_eq!(
        cases,
        [
            "error",
            "models",
            "select_model",
            "status",
            "switch_profile",
            "workbench",
        ],
        "both suites pin exactly the same case list, so a case added on \
         one side fails the other"
    );
}

#[test]
fn server_to_client_workshop_frames_match_the_shared_fixture() {
    // Each typed frame serializes to its fixture entry, compared as
    // values so key order in the file is free.
    let fixture = workshop_fixture();
    assert_eq!(
        serde_json::to_value(
            StatusBarUpdate {
                label: "Ready".to_owned(),
                description: "idle".to_owned(),
                busy: false,
                severity: Severity::Info,
                activity: Activity::General,
            }
            .frame(),
        )
        .expect("the frame serializes"),
        fixture["status"]
    );
    assert_eq!(
        serde_json::to_value(
            CatalogPush {
                models: vec![serde_json::json!({"id": "test-model", "object": "model"})],
            }
            .frame(),
        )
        .expect("the frame serializes"),
        fixture["models"]
    );
    assert_eq!(
        serde_json::to_value(
            WorkbenchSnapshot {
                profiles: vec!["main".to_owned(), "coding".to_owned()],
                active: Some("main".to_owned()),
                switching: None,
                switch_in_flight: false,
                selected_model: Some("test-model".to_owned()),
                chat_ready: true,
            }
            .frame(),
        )
        .expect("the frame serializes"),
        fixture["workbench"]
    );
    let id = serde_json::json!(3);
    assert_eq!(
        serde_json::to_value(ErrorFrame::new("unknown model".to_owned(), Some(&id)))
            .expect("the frame serializes"),
        fixture["error"]
    );
}

#[test]
fn client_to_server_workshop_frames_match_the_shared_fixture() {
    // Both inbound frames parse through their typed bodies, which ignore
    // the envelope tag and the optional `id` the session echoes.
    let fixture = workshop_fixture();
    let select: SelectModelFrame = serde_json::from_value(fixture["select_model"].clone())
        .expect("the fixture select_model parses");
    assert_eq!(select.model, "test-model");
    let switch: SwitchProfileFrame = serde_json::from_value(fixture["switch_profile"].clone())
        .expect("the fixture switch_profile parses");
    assert_eq!(switch.name.as_deref(), Some("beta"));
}
