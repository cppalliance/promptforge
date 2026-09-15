use super::*;

use tokio::sync::broadcast::error::{RecvError, TryRecvError};

#[path = "menu-tests-memory.rs"]
mod memory;

/// A catalog bus already holding one push of the given model ids.
fn catalog_of(ids: &[&str]) -> CatalogBus {
    let catalog = CatalogBus::new();
    catalog.publish(ids.iter().map(|id| serde_json::json!({"id": id})).collect());
    catalog
}

/// A menu with no persistence, on a catalog of the given model ids.
fn menu_of(ids: &[&str]) -> MenuBus {
    MenuBus::new(catalog_of(ids), None)
}

/// Drives `menu` to full readiness on `profile`: gateway reachable
/// and a completed switch, which selects a model.
fn onto_profile(menu: &MenuBus, profile: &str) {
    menu.set_gateway_reachable(true);
    menu.begin_switch(Some(profile))
        .expect("no switch is running");
    menu.finish_switch(SwitchOutcome::Completed);
}

/// The retained snapshot, which every mutation publishes.
fn snapshot(menu: &MenuBus) -> WorkbenchSnapshot {
    menu.latest().expect("a mutation published a snapshot")
}

#[test]
fn a_known_model_selects_and_publishes_the_snapshot() {
    let menu = menu_of(&["model-a", "model-b"]);
    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    assert_eq!(snapshot(&menu).selected_model.as_deref(), Some("model-b"));
}

#[test]
fn an_unknown_model_is_refused_and_not_applied() {
    let menu = menu_of(&["model-a"]);
    menu.set_selected("model-a")
        .expect("the id is in the catalog");
    let refusal = menu
        .set_selected("model-x")
        .expect_err("an unknown id is refused");
    assert_eq!(
        refusal,
        MenuRefusal::UnknownModel {
            id: "model-x".to_string()
        }
    );
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-a"),
        "a refused selection leaves the applied one in place"
    );
}

#[test]
fn a_switch_publishes_its_begin_and_its_finish() {
    let menu = menu_of(&["model-a"]);
    menu.set_gateway_reachable(true);
    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    let during = snapshot(&menu);
    assert_eq!(during.switching.as_deref(), Some("coding"));
    assert!(during.switch_in_flight, "a named switch is in flight");
    assert!(!during.chat_ready, "a switch in flight blocks chat");
    menu.finish_switch(SwitchOutcome::Completed);
    let after = snapshot(&menu);
    assert_eq!(after.active.as_deref(), Some("coding"));
    assert_eq!(after.switching, None);
    assert!(
        !after.switch_in_flight,
        "the finish clears the in-flight mark"
    );
    assert_eq!(
        after.selected_model.as_deref(),
        Some("model-a"),
        "with no memory for the profile the first catalog model is selected"
    );
    assert!(after.chat_ready);
}

#[test]
fn a_failed_switch_keeps_the_previous_profile() {
    let menu = menu_of(&["model-a"]);
    onto_profile(&menu, "main");
    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    menu.finish_switch(SwitchOutcome::Failed);
    let after = snapshot(&menu);
    assert_eq!(after.active.as_deref(), Some("main"));
    assert_eq!(after.switching, None);
    assert!(after.chat_ready, "the previous profile still serves");
}

#[test]
fn a_second_switch_while_one_runs_is_refused() {
    let menu = menu_of(&["model-a"]);
    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    let refusal = menu
        .begin_switch(Some("writing"))
        .expect_err("switches are single-flight");
    assert_eq!(
        refusal,
        MenuRefusal::SwitchInProgress {
            name: Some("coding".to_string())
        }
    );
    assert_eq!(
        refusal.to_string(),
        "a switch to \"coding\" is already in progress",
        "the refusal names the running switch's target"
    );
    assert_eq!(
        snapshot(&menu).switching.as_deref(),
        Some("coding"),
        "the refused switch is not applied"
    );
}

#[test]
fn a_switch_to_no_profile_blocks_chat_and_settles_with_no_active_profile() {
    let menu = menu_of(&["model-a", "model-b"]);
    onto_profile(&menu, "main");
    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    menu.begin_switch(None).expect("no switch is running");
    let during = snapshot(&menu);
    assert_eq!(
        during.switching, None,
        "the wire frame names no target while no profile is being selected"
    );
    assert!(
        during.switch_in_flight,
        "the frame still reports a switch to no profile as in flight"
    );
    assert!(!during.chat_ready, "a switch in flight blocks chat");
    assert_eq!(
        during.active.as_deref(),
        Some("main"),
        "the previous profile still serves while the switch runs"
    );
    let refusal = menu
        .begin_switch(Some("coding"))
        .expect_err("switches are single-flight even toward no profile");
    assert_eq!(refusal, MenuRefusal::SwitchInProgress { name: None });
    assert_eq!(
        refusal.to_string(),
        "a switch to no profile is already in progress"
    );
    menu.finish_switch(SwitchOutcome::Completed);
    let after = snapshot(&menu);
    assert_eq!(after.active, None, "no profile is active once it settles");
    assert_eq!(after.switching, None);
    assert!(
        !after.switch_in_flight,
        "the finish clears the in-flight mark"
    );
    assert_eq!(
        after.selected_model.as_deref(),
        Some("model-a"),
        "with no profile there is no memory; the first catalog model serves"
    );
    assert!(after.chat_ready, "chat is usable with no profile selected");
}

#[test]
fn a_deferred_switch_keeps_the_previous_profile_and_clears_the_switch() {
    let menu = menu_of(&["model-a"]);
    onto_profile(&menu, "main");
    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    menu.finish_switch(SwitchOutcome::Deferred);
    let after = snapshot(&menu);
    assert_eq!(
        after.active.as_deref(),
        Some("main"),
        "the selection persisted but the running profile is unchanged"
    );
    assert_eq!(after.switching, None);
    assert!(after.chat_ready, "the previous profile still serves");
}

#[test]
fn finishing_with_no_switch_in_flight_is_tolerated() {
    let menu = menu_of(&[]);
    menu.finish_switch(SwitchOutcome::Completed);
    assert!(menu.latest().is_none(), "a no-op finish publishes nothing");
}

#[test]
fn the_newest_snapshot_is_retained_for_the_connect_snapshot() {
    let menu = menu_of(&["model-a", "model-b"]);
    assert!(menu.latest().is_none(), "an untouched menu has no snapshot");
    menu.set_selected("model-a")
        .expect("the id is in the catalog");
    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    assert_eq!(
        snapshot(&menu).selected_model.as_deref(),
        Some("model-b"),
        "a session connecting now snapshots the newest state"
    );
}

#[tokio::test]
async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
    let menu = menu_of(&["model-a"]);
    let mut receiver = menu.subscribe();
    for _ in 0..=MENU_CHANNEL_CAPACITY {
        menu.set_gateway_reachable(true);
    }
    match receiver.recv().await {
        Err(RecvError::Lagged(1)) => {}
        other => panic!("expected a lag report of one, got {other:?}"),
    }
    receiver
        .recv()
        .await
        .expect("the ring still holds snapshots");
}

#[test]
fn a_catalog_change_clears_a_selection_it_no_longer_holds() {
    let catalog = catalog_of(&["model-a"]);
    let menu = MenuBus::new(catalog.clone(), None);
    menu.set_selected("model-a")
        .expect("the id is in the catalog");
    catalog.publish(vec![serde_json::json!({"id": "model-b"})]);
    menu.reconcile_catalog();
    assert_eq!(
        snapshot(&menu).selected_model,
        None,
        "the vanished selection is revalidated away"
    );
}

#[test]
fn a_catalog_change_that_keeps_the_selection_republishes_nothing() {
    let catalog = catalog_of(&["model-a"]);
    let menu = MenuBus::new(catalog.clone(), None);
    menu.set_selected("model-a")
        .expect("the id is in the catalog");
    let mut receiver = menu.subscribe();
    catalog.publish(vec![
        serde_json::json!({"id": "model-a"}),
        serde_json::json!({"id": "model-b"}),
    ]);
    menu.reconcile_catalog();
    assert!(
        matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
        "an unchanged snapshot is not republished"
    );
}

#[test]
fn chat_ready_is_true_only_when_every_condition_holds() {
    let catalog = catalog_of(&["model-a"]);
    let menu = MenuBus::new(catalog.clone(), None);
    onto_profile(&menu, "main");
    assert!(
        snapshot(&menu).chat_ready,
        "non-empty catalog, a selection, no switch, gateway up"
    );

    menu.set_gateway_reachable(false);
    assert!(!snapshot(&menu).chat_ready, "gateway down forces false");
    menu.set_gateway_reachable(true);

    menu.begin_switch(Some("coding"))
        .expect("no switch is running");
    assert!(
        !snapshot(&menu).chat_ready,
        "a switch in flight forces false"
    );
    menu.finish_switch(SwitchOutcome::Completed);
    assert!(
        snapshot(&menu).chat_ready,
        "the finished switch restores readiness"
    );

    catalog.publish(vec![serde_json::json!({"id": "model-b"})]);
    menu.reconcile_catalog();
    let cleared = snapshot(&menu);
    assert_eq!(cleared.selected_model, None);
    assert!(!cleared.chat_ready, "no selection forces false");

    menu.set_selected("model-b")
        .expect("the id is in the catalog");
    catalog.publish(Vec::new());
    menu.reconcile_catalog();
    assert!(!snapshot(&menu).chat_ready, "an empty catalog forces false");
}

#[test]
fn chat_ready_is_false_before_the_heartbeat_reports_reachability() {
    let menu = menu_of(&["model-a"]);
    menu.set_selected("model-a")
        .expect("the id is in the catalog");
    assert!(
        !snapshot(&menu).chat_ready,
        "a fresh menu boots unreachable: catalog and selection alone \
         must not open chat before the heartbeat's first verdict"
    );
}

#[test]
fn set_profiles_publishes_the_list_and_the_active_profile() {
    let menu = menu_of(&["model-a"]);
    menu.set_profiles(
        vec!["main".to_string(), "coding".to_string()],
        Some("main".to_string()),
    );
    let published = snapshot(&menu);
    assert_eq!(published.profiles, ["main", "coding"]);
    assert_eq!(published.active.as_deref(), Some("main"));
}

#[test]
fn set_profiles_leaves_the_selection_and_readiness_alone() {
    let menu = menu_of(&["model-a"]);
    onto_profile(&menu, "main");
    menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
    let after = snapshot(&menu);
    assert_eq!(
        after.selected_model.as_deref(),
        Some("model-a"),
        "the profile list does not own selection validity"
    );
    assert!(after.chat_ready, "readiness survives a profile refresh");
}

#[test]
fn an_empty_profile_list_replaces_a_populated_one() {
    let menu = menu_of(&[]);
    menu.set_profiles(vec!["main".to_string()], Some("main".to_string()));
    menu.set_profiles(Vec::new(), None);
    let after = snapshot(&menu);
    assert!(
        after.profiles.is_empty(),
        "a gateway without profile support publishes an empty list"
    );
    assert_eq!(after.active, None);
}
