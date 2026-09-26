//! The `test-fixtures` write stall: an armed stall holds exactly the
//! next write until the test releases it, and the write then lands.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

/// How long an unstalled write may take before the test calls it held.
const UNHELD_WRITE_LIMIT: Duration = Duration::from_secs(10);

#[test]
fn an_armed_stall_holds_only_the_next_write_until_released() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("late.txt");
    let stall = workspace.stall_next_write_for_test();

    let writer = {
        let workspace = workspace.clone();
        let file = file.clone();
        thread::spawn(move || workspace.write_file(&file, "late", None))
    };
    thread::sleep(Duration::from_millis(50));
    assert!(
        !writer.is_finished(),
        "the stalled write waits for its release"
    );
    assert!(!file.exists(), "nothing lands before the release");

    stall.release();
    stall.await_completion();
    assert_eq!(
        fs::read_to_string(&file).expect("the released write landed"),
        "late"
    );
    let written = writer
        .join()
        .expect("the writer thread does not panic")
        .expect("the released write succeeds");

    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = done_tx.send(workspace.write_file(&file, "next", Some(&written.token)));
    });
    let next = done_rx
        .recv_timeout(UNHELD_WRITE_LIMIT)
        .expect("the stall arms one write, so the next goes straight through");
    assert_eq!(next.expect("the next write succeeds").text, "next");
}
