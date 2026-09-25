//! Per-profile model memory: remembered selections across switches, the
//! state file's round trip and its tolerated failures, latest-wins
//! writes, and the boot-time selection restore.

use super::*;

use super::super::memory::store_memory;

#[test]
fn a_selection_is_remembered_per_profile_across_switches() {
    let menu = menu_of(&["model-a", "model-b", "model-c"]);
    onto_profile(&menu, "main");
    menu.set_selected("model-c")
        .expect("the id is in the catalog");
    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    menu.finish_switch(SwitchOutcome::Completed);
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "a profile with no memory selects the first model"
    );
    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    menu.begin_switch(Some("main"))
        .expect("no switch is running");
    menu.finish_switch(SwitchOutcome::Completed);
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-c"),
        "the remembered model for the profile is restored"
    );
}

#[test]
fn model_memory_round_trips_through_the_state_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let catalog = catalog_of(&["model-a", "model-b"]);
    {
        let menu = MenuBus::new(catalog.clone(), Some(dir.path()));
        onto_profile(&menu, "main");
        menu.set_selected("model-b")
            .expect("the id is in the catalog");
    }
    let reborn = MenuBus::new(catalog, Some(dir.path()));
    onto_profile(&reborn, "main");
    assert_eq!(
        snapshot(&reborn).selected_model.as_deref(),
        Some("model-b"),
        "the persisted memory survives a restart"
    );
}

#[test]
fn a_missing_state_file_means_no_memory_yet() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
    onto_profile(&menu, "main");
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "no memory yet: the first catalog model is selected"
    );
}

#[test]
fn a_corrupt_state_file_means_no_memory_yet() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join(WORKSHOP_STATE_FILE), "not json {").expect("write fixture");
    let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
    onto_profile(&menu, "main");
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "corrupt memory degrades to no memory, never to a failure"
    );
}

#[test]
fn an_unreadable_state_file_means_no_memory_yet() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory in the file's place: reads and writes both fail for
    // a reason other than NotFound, and both must degrade.
    std::fs::create_dir(dir.path().join(WORKSHOP_STATE_FILE))
        .expect("directory in the file's place");
    let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
    onto_profile(&menu, "main");
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "unreadable memory degrades to no memory, never to a failure"
    );
}

#[test]
fn writes_landing_newest_first_leave_the_newest_snapshot_on_disk() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join(WORKSHOP_STATE_FILE);
    let mut writer = MemoryWriter::new(path.clone());
    let older = writer.pending(br#"{"last_selected":{"main":"model-a"}}"#.to_vec());
    let newer = writer.pending(br#"{"last_selected":{"main":"model-b"}}"#.to_vec());
    store_memory(&newer);
    store_memory(&older);
    assert_eq!(
        std::fs::read_to_string(&path).expect("the newer snapshot was written"),
        r#"{"last_selected":{"main":"model-b"}}"#,
        "an older snapshot landing last never overwrites the newest"
    );
}

#[test]
fn a_remembered_model_gone_from_the_catalog_falls_back_to_the_first() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join(WORKSHOP_STATE_FILE),
        r#"{"last_selected":{"main":"retired-model"}}"#,
    )
    .expect("write fixture");
    let menu = MenuBus::new(catalog_of(&["model-a"]), Some(dir.path()));
    onto_profile(&menu, "main");
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "a remembered model the catalog no longer holds falls back to the first"
    );
}

#[test]
fn restore_selection_picks_the_remembered_model_for_the_active_profile() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join(WORKSHOP_STATE_FILE),
        r#"{"last_selected":{"main":"model-b"}}"#,
    )
    .expect("write fixture");
    let menu = MenuBus::new(catalog_of(&["model-a", "model-b"]), Some(dir.path()));
    menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
    menu.restore_selection();
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-b"),
        "boot restores the remembered model for the active profile"
    );
}

#[test]
fn restore_selection_falls_back_to_the_first_model_when_memory_is_stale() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join(WORKSHOP_STATE_FILE),
        r#"{"last_selected":{"main":"retired-model"}}"#,
    )
    .expect("write fixture");
    let menu = MenuBus::new(catalog_of(&["model-a", "model-b"]), Some(dir.path()));
    menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
    menu.restore_selection();
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "a remembered model the catalog lacks falls back to the first"
    );
}

#[test]
fn restore_selection_without_an_active_profile_picks_the_first_model() {
    let menu = menu_of(&["model-a", "model-b"]);
    menu.restore_selection();
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "with no active profile there is no memory; the first model serves"
    );
}

#[test]
fn restore_selection_with_a_selection_applied_publishes_nothing() {
    let menu = menu_of(&["model-a", "model-b"]);
    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    let mut receiver = menu.subscribe();
    menu.restore_selection();
    assert!(
        matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
        "an existing selection makes the restore a no-op"
    );
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-b"),
        "the surviving selection is untouched"
    );
}

#[test]
fn restore_selection_with_an_empty_catalog_publishes_nothing() {
    let menu = menu_of(&[]);
    let mut receiver = menu.subscribe();
    menu.restore_selection();
    assert!(
        matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
        "an empty catalog leaves nothing to restore"
    );
    assert!(
        menu.latest().is_none(),
        "a no-op restore retains no snapshot"
    );
}
