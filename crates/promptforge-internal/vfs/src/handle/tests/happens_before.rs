//! Tests for scopes, forks, and joins: which claims the happens-before
//! order lets through, and the prune that keeps the tables bounded.

use super::*;

#[test]
fn dropping_an_access_releases_its_identity_and_claims() -> Result<(), VfsError> {
    let stub = StubFs::default();
    let vfs = handle(&stub);
    let first = vfs.acquire(test_origin())?;
    let first_id = first.id;
    first.write("/f.txt", b"1")?;
    drop(first);
    assert!(stub.released().contains(&first_id));
    let second = vfs.acquire(test_origin())?;
    second.write("/f.txt", b"2")?;
    assert_eq!(second.read("/f.txt")?, b"2");
    Ok(())
}

#[test]
fn an_ended_scopes_claims_are_ignored() -> Result<(), VfsError> {
    // The scope model's release rule: a scope's claims die with its
    // last identity, so a dropped sole identity never blocks the next
    // scope. Were the ended scope's claims still live, this write
    // would conflict.
    let vfs = handle(&StubFs::default());
    let first = vfs.acquire(test_origin())?;
    first.write("/f.txt", b"1")?;
    drop(first);
    let second = vfs.acquire(test_origin())?;
    second.write("/f.txt", b"2")?;
    assert_eq!(second.read("/f.txt")?, b"2");
    Ok(())
}

#[test]
fn a_child_sees_its_parents_pre_spawn_writes() -> Result<(), VfsError> {
    // The fork's snapshot: everything the parent did before the
    // spawn happens before the child's first step, so the child can
    // touch the same path without a false conflict.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    parent.write("/f.txt", b"1")?;
    let child = parent.spawn(test_origin())?;
    assert_ne!(parent.id, child.id);
    child.write("/f.txt", b"2")?;
    assert_eq!(child.read("/f.txt")?, b"2");
    Ok(())
}

#[test]
fn a_child_of_a_directly_wrapped_handle_reads_its_parents_pre_spawn_write() -> Result<(), VfsError>
{
    // The wrapped handle keeps its own claims table, so the child
    // must reach it in the parent's scope or the read conflicts.
    let vfs = VfsRef::new(handle(&StubFs::default()));
    let parent = vfs.acquire(test_origin())?;
    parent.write("/p", b"1")?;
    let child = parent.spawn(test_origin())?;
    assert_eq!(child.read("/p")?, b"1");
    Ok(())
}

#[test]
fn a_forked_child_shares_its_parents_clock_snapshot_instead_of_copying_it() -> Result<(), VfsError>
{
    // The Memory item: a spawn reuses the parent's seen snapshot -
    // one Arc clone - instead of copying the whole map, so a fanout
    // of N holds N views over one shared map rather than N copies of
    // size O(N).
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    parent.write("/seed.txt", b"1")?;
    let child = parent.spawn(test_origin())?;
    let shared = {
        let inner = parent.scope.lock();
        let parent_identity = inner
            .identities
            .get(&parent.id)
            .expect("the parent's identity is registered");
        let child_identity = inner
            .identities
            .get(&child.id)
            .expect("the child's identity is registered");
        Arc::ptr_eq(&parent_identity.seen, &child_identity.seen)
    };
    assert!(shared, "the child reuses the parent's seen snapshot Arc");
    Ok(())
}

#[test]
fn an_ended_identity_keeps_no_clock_snapshot() -> Result<(), VfsError> {
    // The parent joins an arm before the spawn, so the snapshot the
    // child forks is not empty.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let arm = parent.spawn(test_origin())?;
    arm.write("/arm.txt", b"arm")?;
    let arm_id = arm.id;
    drop(arm);
    parent.join(arm_id);
    let child = parent.spawn(test_origin())?;
    child.write("/child.txt", b"child")?;
    let child_id = child.id;
    drop(child);
    parent.scope.end(Some(parent.id), child_id);
    {
        let inner = parent.scope.lock();
        let child_identity = inner
            .identities
            .get(&child_id)
            .expect("the child's record stays");
        assert!(
            child_identity.seen.is_empty(),
            "the ended child holds a snapshot of {} entries",
            child_identity.seen.len()
        );
    }
    assert_eq!(parent.read("/child.txt")?, b"child");
    Ok(())
}

#[test]
fn a_parents_post_spawn_write_conflicts_with_the_child_reading_it() -> Result<(), VfsError> {
    // The fork's other half: the parent's entry advanced at the
    // spawn, so a write after it is unordered with the child's reads.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let child = parent.spawn(test_origin())?;
    parent.write("/f.txt", b"1")?;
    let message = conflict_message(child.read("/f.txt"));
    assert!(message.contains("/f.txt"), "names the path: {message}");
    Ok(())
}

#[test]
fn a_join_makes_a_finished_arms_writes_readable() -> Result<(), VfsError> {
    // The fanout pattern happens-before teaches: the parent joins
    // each arm in turn, and the join orders the arm's writes before
    // the next arm's first step.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let arm_one = parent.spawn(test_origin())?;
    arm_one.write("/evidence.md", b"one\n")?;
    let arm_one_id = arm_one.id;
    drop(arm_one);
    parent.join(arm_one_id);
    let arm_two = parent.spawn(test_origin())?;
    arm_two.append("/evidence.md", b"two\n")?;
    assert_eq!(arm_two.read("/evidence.md")?, b"one\ntwo\n");
    Ok(())
}

#[test]
fn a_sibling_read_of_a_siblings_write_conflicts() -> Result<(), VfsError> {
    // The unordered read-write pair: one arm writes a file, a sibling
    // reads it, and nothing ordered the two arms, so the read fails
    // in every interleaving.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let writer = parent.spawn(test_origin())?;
    writer.write("/research/5.md", b"five")?;
    let reader = parent.spawn(test_origin())?;
    let message = conflict_message(reader.read("/research/5.md"));
    assert!(
        message.contains("/research/5.md"),
        "names the path: {message}"
    );
    Ok(())
}

#[test]
fn a_task_forked_after_a_write_reads_it_and_one_forked_before_conflicts() -> Result<(), VfsError> {
    // The fork's ordering: A's clock was forked before the owner's
    // write, so A's read always conflicts; B was forked after, so B's
    // read always passes.
    let vfs = handle(&StubFs::default());
    let owner = vfs.acquire(test_origin())?;
    let a = owner.spawn(test_origin())?;
    owner.write("/x.txt", b"1")?;
    let b = owner.spawn(test_origin())?;
    let message = conflict_message(a.read("/x.txt"));
    assert!(message.contains("/x.txt"), "A conflicts: {message}");
    assert_eq!(b.read("/x.txt")?, b"1");
    Ok(())
}

#[test]
fn a_seeding_scope_and_the_runs_scope_do_not_conflict() -> Result<(), VfsError> {
    // Harness seeding and the run are separate scopes: the seeding
    // scope ends with its access, so the run reads freely.
    let vfs = handle(&StubFs::default());
    let seeding = vfs.acquire(test_origin())?;
    seeding.write("/brief.md", b"seeded")?;
    drop(seeding);
    let run = vfs.acquire(test_origin())?;
    assert_eq!(run.read("/brief.md")?, b"seeded");
    Ok(())
}

#[test]
fn two_live_scopes_writing_one_path_conflict() -> Result<(), VfsError> {
    // Two concurrent runs share a real-directory base: their scopes are both
    // live and nothing orders two scopes, so the second write
    // conflicts exactly as it did under the liveness model.
    let vfs = handle(&StubFs::default());
    let first = vfs.acquire(test_origin())?;
    first.write("/shared.txt", b"1")?;
    let second = vfs.acquire(test_origin())?;
    let message = conflict_message(second.write("/shared.txt", b"2"));
    assert!(message.contains("/shared.txt"), "names the path: {message}");

    // A prune in between keeps the write conflicting, for a scope
    // acquired before the prune and for one acquired after it.
    for acquired_before in [true, false] {
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire(test_origin())?;
        first.write("/shared.txt", b"1")?;
        let early = if acquired_before {
            Some(vfs.acquire(test_origin())?)
        } else {
            None
        };
        for n in 0..=PRUNE_AT {
            first.exists(&format!("/other/{n}.txt"))?;
        }
        let second = match early {
            Some(second) => second,
            None => vfs.acquire(test_origin())?,
        };
        let message = conflict_message(second.write("/shared.txt", b"2"));
        assert!(
            message.contains("/shared.txt"),
            "acquired before the prune: {acquired_before}: {message}"
        );
    }
    Ok(())
}

#[test]
fn a_prune_drops_emptied_regions_and_raises_its_threshold() -> Result<(), VfsError> {
    // A dead scope's claims go at the next prune, and so do the
    // regions they leave empty; the next threshold is measured from
    // what survives.
    let vfs = handle(&StubFs::default());
    let spent = vfs.acquire(test_origin())?;
    for n in 0..PRUNE_AT {
        spent.exists(&format!("/p{n}"))?;
    }
    drop(spent);
    let fresh = vfs.acquire(test_origin())?;
    // The claim past the threshold, which prunes.
    fresh.exists("/fresh")?;
    let tables = vfs.volume.claims.tables();
    assert_eq!(
        tables.paths.len(),
        1,
        "only the live scope's region survives"
    );
    assert!(
        tables.prune_at > tables.entries,
        "the next prune waits for growth: threshold {}, entries {}",
        tables.prune_at,
        tables.entries
    );
    drop(tables);

    // When more than half the threshold survives, the next prune
    // waits for the survivors to double.
    let vfs = handle(&StubFs::default());
    let keeper = vfs.acquire(test_origin())?;
    for n in 0..=PRUNE_AT / 2 {
        keeper.exists(&format!("/keep{n}"))?;
    }
    let spent = vfs.acquire(test_origin())?;
    for n in 0..PRUNE_AT - PRUNE_AT / 2 - 1 {
        spent.exists(&format!("/p{n}"))?;
    }
    drop(spent);
    keeper.exists("/trigger")?;
    let tables = vfs.volume.claims.tables();
    assert_eq!(tables.entries, PRUNE_AT / 2 + 2, "the live claims survive");
    assert_eq!(
        tables.prune_at,
        2 * tables.entries,
        "the threshold doubles the survivors"
    );
    Ok(())
}

#[test]
fn a_prune_collapses_an_epoch_that_only_a_held_ended_identity_never_saw() -> Result<(), VfsError> {
    // The call is still held, but an ended identity claims nothing
    // again, so the prune need not wait for it to see the write.
    let vfs = handle(&StubFs::default());
    let owner = vfs.acquire(test_origin())?;
    let call = owner.spawn(test_origin())?;
    owner.write("/after.txt", b"1")?;
    owner.scope.end(Some(owner.id), call.id);
    for n in 0..PRUNE_AT {
        owner.exists(&format!("/p{n}"))?;
    }
    let tables = vfs.volume.claims.tables();
    let write = tables
        .paths
        .iter()
        .find(|(path, _)| path.as_str() == "/after.txt")
        .and_then(|(_, region)| region.write)
        .expect("the owner's write is recorded");
    assert_eq!(write.1.clock, 0, "the prune collapses the owner's write");
    drop(tables);
    drop(call);
    Ok(())
}
