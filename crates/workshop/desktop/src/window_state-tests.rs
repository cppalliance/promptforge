//! Window geometry without a window: the physical-to-logical mapping the
//! saver applies under a scale factor, the maximized merge that keeps the
//! normal rectangle, the empty-size guard on a saved geometry, the debounce
//! that turns a drag's burst of events into one save, and the wire shape
//! the server's workspace-file routes speak.

use std::sync::mpsc;
use std::time::Duration;

use tauri::{PhysicalPosition, PhysicalSize};

use super::{CurrentResponse, Debouncer, SavedResponse, WindowState, geometry, merge, usable};

/// How long a test waits for the debouncer to deliver a coalesced save.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// The debounce a test runs with: long enough that a burst pushed in a
/// tight loop lands inside one window, short enough to keep the test fast.
const TEST_DEBOUNCE: Duration = Duration::from_millis(150);

fn state(width: u32, height: u32, x: i32, y: i32, maximized: bool) -> WindowState {
    WindowState {
        width,
        height,
        x,
        y,
        maximized,
    }
}

#[test]
fn physical_geometry_maps_to_logical_under_the_scale_factor() {
    // A 200% display: the OS reports twice the logical pixels in every
    // axis, and the file keeps logical pixels so a 100% display restores
    // the same apparent size.
    let mapped = geometry(
        PhysicalSize::new(2560, 1600),
        PhysicalPosition::new(200, -100),
        2.0,
        false,
    );
    assert_eq!(mapped, state(1280, 800, 100, -50, false));

    // At 100% the mapping is the identity.
    let identity = geometry(
        PhysicalSize::new(1024, 768),
        PhysicalPosition::new(40, 60),
        1.0,
        false,
    );
    assert_eq!(identity, state(1024, 768, 40, 60, false));

    // The maximized flag passes through untouched.
    let maximized = geometry(
        PhysicalSize::new(3840, 2100),
        PhysicalPosition::new(-8, -8),
        1.5,
        true,
    );
    assert!(maximized.maximized);
    assert_eq!((maximized.width, maximized.height), (2560, 1400));
}

#[test]
fn a_maximized_snapshot_keeps_the_last_normal_rectangle() {
    let normal = state(1280, 800, 100, 50, false);
    let maximized_rect = state(2560, 1400, -8, -8, true);

    // Maximizing reports the screen rectangle; the file keeps the normal
    // one so un-maximizing after a restart lands where the user left it.
    assert_eq!(
        merge(Some(normal), maximized_rect),
        state(1280, 800, 100, 50, true)
    );
    // Un-maximizing takes the fresh rectangle again.
    let moved = state(1000, 700, 300, 200, false);
    assert_eq!(merge(Some(state(1280, 800, 100, 50, true)), moved), moved);
    // With nothing remembered, the maximized rectangle is all there is.
    assert_eq!(merge(None, maximized_rect), maximized_rect);
}

#[test]
fn a_saved_geometry_with_an_empty_side_is_not_applied() {
    assert!(
        !usable(state(0, 800, 100, 50, false)),
        "a zero width is empty"
    );
    assert!(
        !usable(state(1280, 0, 100, 50, false)),
        "a zero height is empty"
    );
    assert!(
        !usable(state(0, 0, 0, 0, true)),
        "a zero size is empty even when maximized"
    );
    assert!(
        usable(state(1, 1, -32_000, -32_000, false)),
        "any nonzero size is usable; the position is checked against the monitors separately"
    );
}

#[test]
fn a_burst_of_geometry_events_coalesces_into_one_save() {
    let (sink, saved) = mpsc::channel();
    let debouncer = Debouncer::spawn(TEST_DEBOUNCE, move |value: WindowState| {
        sink.send(value).expect("the test holds the receiver");
    });

    // A drag: many events inside the debounce window.
    for x in 0..50 {
        debouncer.push(state(1280, 800, x, 0, false));
    }

    let first = saved
        .recv_timeout(DELIVERY_TIMEOUT)
        .expect("the burst produces one save");
    assert_eq!(first, state(1280, 800, 49, 0, false), "the last value wins");
    assert!(
        saved.recv_timeout(TEST_DEBOUNCE * 3).is_err(),
        "no intermediate value reaches the sink"
    );

    // A second, separate gesture saves again.
    debouncer.push(state(1000, 700, 5, 5, true));
    let second = saved
        .recv_timeout(DELIVERY_TIMEOUT)
        .expect("a later gesture produces its own save");
    assert_eq!(second, state(1000, 700, 5, 5, true));
}

#[test]
fn the_wire_shape_matches_the_server_fixture() {
    // Copied from the server's `WindowState` (the kv 'window' value and
    // the `PUT /workspace/file/window-state` body) and the
    // `GET /workspace/file/current` answer.
    let request = serde_json::to_value(state(1280, 800, 100, -50, false))
        .expect("a five-field struct of plain values serializes");
    assert_eq!(
        request,
        serde_json::json!({
            "width": 1280,
            "height": 800,
            "x": 100,
            "y": -50,
            "maximized": false
        })
    );

    let current: CurrentResponse = serde_json::from_str(
        r#"{"path":"C:\\ws\\Name.pfwork","name":"Name","grants":[{"path":"C:\\src","exists":true}],"window_state":{"width":1280,"height":800,"x":100,"y":-50,"maximized":true}}"#,
    )
    .expect("the current-file answer parses");
    assert_eq!(current.window_state, Some(state(1280, 800, 100, -50, true)));

    let ephemeral: CurrentResponse =
        serde_json::from_str(r#"{"path":null,"name":"Untitled","grants":[],"window_state":null}"#)
            .expect("the ephemeral answer parses");
    assert_eq!(ephemeral.window_state, None);

    let saved: SavedResponse =
        serde_json::from_str(r#"{"saved":false}"#).expect("the window-state answer parses");
    assert!(!saved.saved);
}
