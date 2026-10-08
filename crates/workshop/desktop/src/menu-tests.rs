//! The quit item's accelerator: `CmdOrCtrl+Q` on macOS, where Cmd+Q is the
//! platform's quit gesture, and none on Windows and Linux, where Ctrl+Q
//! opens View in the SPA.

use super::quit_accelerator;

#[test]
fn the_quit_item_keeps_cmd_or_ctrl_q_on_macos() {
    assert_eq!(quit_accelerator(true), Some("CmdOrCtrl+Q"));
}

#[test]
fn the_quit_item_has_no_accelerator_on_windows_and_linux() {
    assert_eq!(quit_accelerator(false), None);
}
