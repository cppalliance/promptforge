//! Tests for the claims between two identities on one path, a copy's and
//! a rename's two paths, claim keys, and claims a refused operation
//! never registers.

use super::*;

#[test]
fn one_identity_never_conflicts_with_itself() -> Result<(), VfsError> {
    // Borrow semantics: a blocking call chain uses the parent's
    // access, so sequential ops on one path by one identity stay
    // legal - no new identity, no false conflict.
    let vfs = handle(&StubFs::default());
    let access = vfs.acquire(test_origin())?;
    access.write("/f.txt", b"one")?;
    access.write("/f.txt", b"two")?;
    access.append("/f.txt", b"!")?;
    assert_eq!(access.read("/f.txt")?, b"two!");
    Ok(())
}

#[test]
fn a_write_conflicts_with_another_identitys_read_claim() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
    let reader = vfs.acquire(test_origin())?;
    let writer = vfs.acquire(test_origin())?;
    reader.read("/f.txt")?;
    let message = conflict_message(writer.write("/f.txt", b"new"));
    assert!(message.contains("/f.txt"), "names the path: {message}");
    assert!(
        message.contains("read"),
        "names the standing claim kind: {message}"
    );
    assert!(
        message.contains("write"),
        "names the attempted kind: {message}"
    );
    assert!(
        message.contains(&format!("{:?}", reader.id)),
        "names the claimant: {message}"
    );
    assert!(
        message.contains(&format!("{:?}", writer.id)),
        "names the attempter: {message}"
    );
    Ok(())
}

#[test]
fn a_read_conflicts_with_another_identitys_write_claim() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let writer = vfs.acquire(test_origin())?;
    writer.write("/f.txt", b"x")?;
    let reader = vfs.acquire(test_origin())?;
    match reader.read("/f.txt") {
        Err(VfsError::Conflict { .. }) => {}
        other => panic!("expected a conflict, got {other:?}"),
    }
    Ok(())
}

#[test]
fn two_writes_by_two_identities_conflict() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let first = vfs.acquire(test_origin())?;
    first.write("/f.txt", b"1")?;
    let second = vfs.acquire(test_origin())?;
    let message = conflict_message(second.write("/f.txt", b"2"));
    assert!(message.contains("write claim"), "{message}");
    Ok(())
}

#[test]
fn reads_by_two_identities_never_conflict() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
    let first = vfs.acquire(test_origin())?;
    let second = vfs.acquire(test_origin())?;
    first.read("/f.txt")?;
    assert_eq!(second.read("/f.txt")?, b"data");
    Ok(())
}

#[test]
fn a_copy_conflicts_with_another_identitys_write_on_the_source() -> Result<(), VfsError> {
    // Copy claims the source as a read, and a read booms on another
    // scope's write claim.
    let vfs = handle(&StubFs::default());
    let writer = vfs.acquire(test_origin())?;
    writer.write("/src.txt", b"data")?;
    let copier = vfs.acquire(test_origin())?;
    let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
    assert!(message.contains("/src.txt"), "names the source: {message}");
    Ok(())
}

#[test]
fn a_copy_shares_the_source_with_another_identitys_read() -> Result<(), VfsError> {
    // The source claim is a read, not a write: another identity's
    // read claim on the source must not block the copy. Were the
    // source claimed as a write, this copy would conflict.
    let vfs = handle(&StubFs::seeded(&[("/src.txt", "data")]));
    let reader = vfs.acquire(test_origin())?;
    reader.read("/src.txt")?;
    let copier = vfs.acquire(test_origin())?;
    copier.copy("/src.txt", "/dst.txt")?;
    assert_eq!(copier.read("/dst.txt")?, b"data");
    Ok(())
}

#[test]
fn a_copy_conflicts_with_another_identitys_claim_on_the_destination() -> Result<(), VfsError> {
    // The destination is claimed as a write, so any other live
    // identity's claim on it blocks the copy.
    let vfs = handle(&StubFs::seeded(&[
        ("/src.txt", "data"),
        ("/dst.txt", "old"),
    ]));
    let reader = vfs.acquire(test_origin())?;
    reader.read("/dst.txt")?;
    let copier = vfs.acquire(test_origin())?;
    let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
    assert!(
        message.contains("/dst.txt"),
        "names the destination: {message}"
    );
    Ok(())
}

#[test]
fn a_rename_conflicts_with_a_claim_on_the_source_path() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/from.txt", "data")]));
    let reader = vfs.acquire(test_origin())?;
    reader.read("/from.txt")?;
    let renamer = vfs.acquire(test_origin())?;
    let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
    assert!(message.contains("/from.txt"), "names the source: {message}");
    Ok(())
}

#[test]
fn a_rename_conflicts_with_a_claim_on_the_destination_path() -> Result<(), VfsError> {
    // Both paths are claimed as writes; were the second gate dropped,
    // this rename would sail through against the standing claim.
    let vfs = handle(&StubFs::seeded(&[
        ("/from.txt", "data"),
        ("/to.txt", "old"),
    ]));
    let reader = vfs.acquire(test_origin())?;
    reader.read("/to.txt")?;
    let renamer = vfs.acquire(test_origin())?;
    let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
    assert!(
        message.contains("/to.txt"),
        "names the destination: {message}"
    );
    Ok(())
}

#[test]
fn transfer_of_control_moves_the_identity_with_the_access() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
    let original = vfs.acquire(test_origin())?;
    original.read("/f.txt")?;
    // Transfer of control moves the access object; the identity - and
    // with it the scope's reference count - moves with it.
    let moved = original;
    let other = vfs.acquire(test_origin())?;
    let message = conflict_message(other.write("/f.txt", b"new"));
    assert!(message.contains(&format!("{:?}", moved.id)));
    assert_eq!(moved.read("/f.txt")?, b"data");
    Ok(())
}

#[test]
fn alias_spellings_of_one_file_land_on_one_claim_key() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/a/b.txt", "x")]));
    let reader = vfs.acquire(test_origin())?;
    reader.read("/a/./b.txt")?;
    let writer = vfs.acquire(test_origin())?;
    let message = conflict_message(writer.write("/a//b.txt", b"y"));
    assert!(message.contains("/a/b.txt"), "the canonical key: {message}");
    Ok(())
}

#[test]
fn claims_are_shared_across_handle_clones() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let clone = vfs.clone();
    let first = vfs.acquire(test_origin())?;
    first.write("/f.txt", b"1")?;
    let second = clone.acquire(test_origin())?;
    let message = conflict_message(second.write("/f.txt", b"2"));
    assert!(message.contains("/f.txt"), "{message}");
    Ok(())
}

#[test]
fn a_denied_operation_never_registers_a_claim() -> Result<(), VfsError> {
    let verdict = Arc::new(Mutex::new(Verdict::Deny("writes are sealed".to_owned())));
    let vfs = VfsRef::with_policy(
        StubFs::default(),
        FlipPolicy {
            verdict: Arc::clone(&verdict),
        },
    );
    let denied = vfs.acquire(test_origin())?;
    match denied.write("/f.txt", b"x") {
        Err(VfsError::PermissionDenied { path, reason }) => {
            assert_eq!(path, "/f.txt");
            assert_eq!(reason, "writes are sealed");
        }
        other => panic!("expected a denial, got {other:?}"),
    }
    // The Host flips the policy mid-run through shared state.
    *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Verdict::Allow;
    let allowed = vfs.acquire(test_origin())?;
    // Had the denied attempt registered a write claim, this write
    // would conflict with it.
    allowed.write("/f.txt", b"x")?;
    assert_eq!(allowed.read("/f.txt")?, b"x");
    Ok(())
}

#[test]
fn a_refused_claim_never_registers_a_claim() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/src.txt", "s"), ("/copied.txt", "c")]));
    let prober = vfs.acquire(test_origin())?;
    prober.exists("/d")?;
    // The write's may-create claim on `/d` races with the probe.
    let refused = vfs.acquire(test_origin())?;
    conflict_message(refused.write("/d/f", b"f"));
    // Had the refused write registered its claim on `/d/f`, this
    // read would conflict with it.
    let reader = vfs.acquire(test_origin())?;
    assert!(
        matches!(reader.read("/d/f"), Err(VfsError::NotFound { .. })),
        "the read reaches the backend"
    );

    // A rename refused on its destination leaves its source
    // unclaimed.
    let renamer = vfs.acquire(test_origin())?;
    conflict_message(renamer.rename("/src.txt", "/d/g"));
    assert_eq!(reader.read("/src.txt")?, b"s");

    // A copy refused on its destination leaves no read claim on its
    // source, so a later unordered write to the source goes through.
    let copier = vfs.acquire(test_origin())?;
    conflict_message(copier.copy("/copied.txt", "/d/h"));
    let writer = vfs.acquire(test_origin())?;
    writer.write("/copied.txt", b"w")?;
    Ok(())
}
