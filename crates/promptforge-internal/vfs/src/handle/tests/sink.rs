//! Tests for the op sink: the events it receives, and the operations
//! that never fire it.

use super::*;

#[test]
fn a_sink_receives_events_in_order_with_op_path_and_label() -> Result<(), VfsError> {
    /// One recorded event: op, canonical path, label, and line.
    type Recorded = (Op, String, String, u32);

    let events: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let vfs = VfsRef::builder()
        .mount("/", StubFs::seeded(&[("/a.txt", "x")]))
        .on_op(move |event: OpEvent<'_>| {
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    event.op(),
                    event.path().to_string(),
                    event.origin().label.clone(),
                    event.origin().line,
                ));
        })
        .build();
    let access = vfs.acquire(Origin::at("the section", "the prompt", 7))?;
    access.read("/a.txt")?;
    access.write("/b.txt", b"y")?;
    access.glob("/*.txt")?;
    // A spawned child's events report the child's own origin.
    let child = access.spawn(Origin::at("the arm", "the prompt", 9))?;
    child.append("/b.txt", b"!")?;
    let events = events.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        *events,
        vec![
            (Op::Read, "/a.txt".to_owned(), "the section".to_owned(), 7),
            (Op::Write, "/b.txt".to_owned(), "the section".to_owned(), 7),
            (Op::Glob, "/*.txt".to_owned(), "the section".to_owned(), 7),
            (Op::Append, "/b.txt".to_owned(), "the arm".to_owned(), 9),
        ]
    );
    Ok(())
}

#[test]
fn a_handle_without_a_sink_serves_operations_without_firing() -> Result<(), VfsError> {
    // The None sink branch: no `on_op`, and operations behave
    // exactly as before - not firing is a no-op, never a panic.
    let vfs = VfsRef::builder().mount("/", StubFs::default()).build();
    let access = vfs.acquire(test_origin())?;
    access.write("/f.txt", b"x")?;
    assert_eq!(access.read("/f.txt")?, b"x");
    Ok(())
}

#[test]
fn a_policy_denied_operation_never_fires_the_sink() -> Result<(), VfsError> {
    let verdict = Arc::new(Mutex::new(Verdict::Deny("writes are sealed".to_owned())));
    let events: Arc<Mutex<Vec<Op>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let vfs = VfsRef::builder()
        .mount("/", StubFs::default())
        .policy(FlipPolicy {
            verdict: Arc::clone(&verdict),
        })
        .on_op(move |event: OpEvent<'_>| {
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event.op());
        })
        .build();
    let access = vfs.acquire(test_origin())?;
    assert!(matches!(
        access.write("/f.txt", b"x"),
        Err(VfsError::PermissionDenied { .. })
    ));
    assert!(
        events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty(),
        "a denied operation fired the sink"
    );
    // The host flips the policy mid-run; the admitted write fires.
    *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Verdict::Allow;
    access.write("/f.txt", b"x")?;
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[Op::Write]
    );
    Ok(())
}
