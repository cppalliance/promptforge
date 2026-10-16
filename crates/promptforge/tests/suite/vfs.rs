//! Guarding and watching a run's files.

use std::sync::{Arc, Mutex};

use promptforge::vfs::{
    MemoryBackend, Mode, ModePolicy, Op, OpEvent, Origin, VfsError, VfsOp, VfsOutcome, VfsRef,
    perform_vfs_op,
};

use super::support::{Scenario, TestResult, note};

fn run_note(vfs: VfsRef) -> TestResult<String> {
    let scenario = Scenario::default().context(move |ctx| ctx.vfs(vfs.clone()));
    Ok(scenario.run(&note("'hello'"))?.text()?.to_owned())
}

#[test]
fn agent_mode_lets_the_run_write_and_ask_mode_refuses_a_later_write() -> TestResult {
    let policy = ModePolicy::new(Mode::Agent);
    let mode = policy.handle();
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .policy(policy)
        .build();

    assert_eq!(run_note(vfs.clone())?, "hello");

    mode.set(Mode::Ask);
    let store = vfs.acquire_store(Origin::new("caller"))?;
    let second = VfsOp::Write {
        path: "note.md".to_owned(),
        contents: "changed".to_owned(),
    };
    let refused = perform_vfs_op(&store, second);
    assert!(
        matches!(&refused, Err(VfsError::ApprovalRequired { path, .. }) if path == "note.md"),
        "{refused:?}"
    );

    let read = VfsOp::Read {
        path: "note.md".to_owned(),
        start: None,
        end: None,
    };
    assert_eq!(
        perform_vfs_op(&store, read)?,
        VfsOutcome::Text("hello".to_owned())
    );
    Ok(())
}

#[test]
fn an_operation_watcher_sees_the_notes_write_then_read_under_one_origin() -> TestResult {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink_log = Arc::clone(&log);
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .on_op(move |event: OpEvent<'_>| {
            if let Ok(mut entries) = sink_log.lock() {
                entries.push((
                    event.op(),
                    event.path().to_string(),
                    event.origin().label.clone(),
                ));
            }
        })
        .build();

    assert_eq!(run_note(vfs)?, "hello");

    let entries = log.lock().map_err(|_| "the watcher panicked")?;
    let note: Vec<_> = entries
        .iter()
        .filter(|(_, path, _)| path == "/note.md")
        .collect();
    assert!(
        matches!(note.as_slice(), [(Op::Write, _, first), (Op::Read, _, second)] if first == second)
    );
    Ok(())
}
