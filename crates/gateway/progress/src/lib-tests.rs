//! The hub's snapshot rule and its `watch` publication.

use gateway_api_types::Progress;

use super::ProgressHub;

fn snapshot(busy: bool, text: &str) -> Progress {
    Progress {
        busy,
        text: text.to_owned(),
    }
}

#[test]
fn a_fresh_hub_is_idle_with_empty_text() {
    let hub = ProgressHub::new();
    assert_eq!(hub.current(), Progress::default());
    assert_eq!(*hub.subscribe().borrow(), Progress::default());
}

#[test]
fn begin_publishes_busy_with_the_activity_text() {
    let hub = ProgressHub::new();
    let _activity = hub.begin("Loading profile");
    assert_eq!(hub.current(), snapshot(true, "Loading profile"));
}

#[test]
fn the_last_drop_publishes_idle() {
    let hub = ProgressHub::new();
    let activity = hub.begin("Loading profile");
    drop(activity);
    assert_eq!(
        hub.current(),
        snapshot(false, ""),
        "an ended activity leaves the hub idle with no stale text"
    );
}

#[test]
fn a_nested_begin_shows_the_newest_and_falls_back_on_drop() {
    let hub = ProgressHub::new();
    let _outer = hub.begin("load-profile: main");
    let inner = hub.begin("Downloading qwen 12%");
    assert_eq!(hub.current(), snapshot(true, "Downloading qwen 12%"));
    drop(inner);
    assert_eq!(
        hub.current(),
        snapshot(true, "load-profile: main"),
        "the enclosing activity's text returns once the nested one ends"
    );
}

#[test]
fn dropping_an_older_activity_keeps_the_newest_text() {
    let hub = ProgressHub::new();
    let outer = hub.begin("outer");
    let _inner = hub.begin("inner");
    drop(outer);
    assert_eq!(
        hub.current(),
        snapshot(true, "inner"),
        "ending an activity that is not the newest changes nothing visible"
    );
}

#[test]
fn set_text_republishes_the_newest_activity() {
    let hub = ProgressHub::new();
    let activity = hub.begin("Downloading qwen 0%");
    activity.set_text("Downloading qwen 45%");
    assert_eq!(hub.current(), snapshot(true, "Downloading qwen 45%"));
}

#[test]
fn set_text_on_an_older_activity_does_not_displace_the_newest() {
    let hub = ProgressHub::new();
    let outer = hub.begin("outer");
    let _inner = hub.begin("inner");
    outer.set_text("outer again");
    assert_eq!(hub.current(), snapshot(true, "inner"));
}

#[tokio::test]
async fn subscribe_receives_each_change() {
    let hub = ProgressHub::new();
    let mut rx = hub.subscribe();
    assert!(
        !rx.has_changed().expect("the sender is alive"),
        "a fresh subscriber has already seen the idle snapshot"
    );

    let activity = hub.begin("Loading profile");
    rx.changed().await.expect("begin publishes");
    assert_eq!(*rx.borrow_and_update(), snapshot(true, "Loading profile"));

    activity.set_text("Downloading models");
    rx.changed().await.expect("set_text publishes");
    assert_eq!(
        *rx.borrow_and_update(),
        snapshot(true, "Downloading models")
    );

    drop(activity);
    rx.changed().await.expect("drop publishes");
    assert_eq!(*rx.borrow_and_update(), snapshot(false, ""));
}

#[test]
fn an_unchanged_snapshot_is_not_republished() {
    let hub = ProgressHub::new();
    let mut rx = hub.subscribe();
    let activity = hub.begin("same");
    assert!(rx.has_changed().expect("the sender is alive"));
    rx.mark_unchanged();
    activity.set_text("same");
    assert!(
        !rx.has_changed().expect("the sender is alive"),
        "a set_text to the identical text wakes no subscriber"
    );
}

#[test]
fn a_subscriber_outlives_the_hub_owner_through_the_activity() {
    let hub = ProgressHub::new();
    let rx = hub.subscribe();
    let activity = hub.begin("held");
    drop(hub);
    // The activity keeps the shared state alive; its drop still publishes.
    drop(activity);
    assert_eq!(*rx.borrow(), snapshot(false, ""));
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProgressHub>();
    assert_send_sync::<super::Activity>();
};
