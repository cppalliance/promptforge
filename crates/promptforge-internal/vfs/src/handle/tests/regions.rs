//! Tests for claims across regions: globs, listings, subtrees, and the
//! ancestors a write may create, racing unordered and ordered by a join.

use super::*;

#[test]
fn a_glob_racing_a_siblings_write_conflicts_in_either_order() -> Result<(), VfsError> {
    // The pattern-overlaps-path rule, both ways round: whichever
    // access runs second detects the overlap between the pattern and
    // the path it matches.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let globber = parent.spawn(test_origin())?;
    let writer = parent.spawn(test_origin())?;
    globber.glob("/research/*")?;
    let message = conflict_message(writer.write("/research/5.md", b"five"));
    assert!(
        message.contains("/research"),
        "the write detects the glob: {message}"
    );

    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let writer = parent.spawn(test_origin())?;
    let globber = parent.spawn(test_origin())?;
    writer.write("/research/5.md", b"five")?;
    let message = conflict_message(globber.glob("/research/*"));
    assert!(
        message.contains("/research"),
        "the glob detects the write: {message}"
    );
    Ok(())
}

#[test]
fn an_exists_racing_a_write_into_the_directory_conflicts() -> Result<(), VfsError> {
    // A write covers the ancestors it may create, so a sibling's
    // `exists` on the directory conflicts however the two land.
    let vfs = handle(&StubFs::default());
    let parent = vfs.acquire(test_origin())?;
    let prober = parent.spawn(test_origin())?;
    let writer = parent.spawn(test_origin())?;
    prober.exists("/research")?;
    let message = conflict_message(writer.write("/research/5.md", b"five"));
    assert!(
        message.contains("/research"),
        "the write detects the probe: {message}"
    );
    Ok(())
}

#[test]
fn a_write_into_a_directory_racing_a_siblings_write_or_remove_of_it_conflicts_in_either_order()
-> Result<(), VfsError> {
    // A write into `/d` may create `/d`, so a sibling's write or
    // plain remove of `/d` itself races with it, whichever lands
    // first.
    type Step = fn(&Access) -> Result<(), VfsError>;
    let into: Step = |access| access.write("/d/f", b"f");
    let on_directory: [(&str, Step); 2] = [
        ("write", |access| access.write("/d", b"d")),
        ("remove", |access| access.remove("/d", false).map(|_| ())),
    ];
    for (name, directory) in on_directory {
        for into_first in [true, false] {
            let vfs = handle(&StubFs::default());
            let parent = vfs.acquire(test_origin())?;
            let first = parent.spawn(test_origin())?;
            let second = parent.spawn(test_origin())?;
            let (lead, trail) = if into_first {
                (into, directory)
            } else {
                (directory, into)
            };
            lead(&first)?;
            let message = conflict_message(trail(&second));
            assert!(
                message.contains("/d"),
                "{name}, the write into /d first: {into_first}: {message}"
            );
        }
    }
    Ok(())
}

type ClaimStep = fn(&Access) -> Result<(), VfsError>;

/// Lists `dir`, counting the stub's refusal to list as success: the
/// claim is taken before the backend is asked.
fn list_claim(access: &Access, dir: &str) -> Result<(), VfsError> {
    match access.list(dir) {
        Ok(_) | Err(VfsError::Unsupported { .. }) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Runs `lead` in one child of a parent and returns `trail`'s result
/// in a sibling. When `joined`, the parent joins the first child
/// before spawning the second, which orders the two.
fn run_pair(
    lead: ClaimStep,
    trail: ClaimStep,
    joined: bool,
) -> Result<Result<(), VfsError>, VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/a/b", "b")]));
    let parent = vfs.acquire(test_origin())?;
    let first = parent.spawn(test_origin())?;
    if joined {
        lead(&first)?;
        let first_id = first.id;
        drop(first);
        parent.join(first_id);
        let second = parent.spawn(test_origin())?;
        return Ok(trail(&second));
    }
    let second = parent.spawn(test_origin())?;
    lead(&first)?;
    Ok(trail(&second))
}

/// The claims-gap pairs that race when unordered, by name.
fn racing_pairs() -> [(&'static str, ClaimStep, ClaimStep); 5] {
    [
        (
            "remove /a against remove /a/b",
            |access| access.remove("/a", true).map(|_| ()),
            |access| access.remove("/a/b", true).map(|_| ()),
        ),
        (
            "remove /a against rename /a/b",
            |access| access.remove("/a", true).map(|_| ()),
            |access| access.rename("/a/b", "/x"),
        ),
        (
            "list /a against remove /a/b",
            |access| list_claim(access, "/a"),
            |access| access.remove("/a/b", true).map(|_| ()),
        ),
        (
            "write /a/b/c against list /a",
            |access| access.write("/a/b/c", b"c"),
            |access| list_claim(access, "/a"),
        ),
        (
            "write /a/b/c against glob /a/*",
            |access| access.write("/a/b/c", b"c"),
            |access| access.glob("/a/*").map(|_| ()),
        ),
    ]
}

#[test]
fn nested_subtrees_parent_listings_and_created_ancestors_conflict_in_either_order()
-> Result<(), VfsError> {
    for (name, one, other) in racing_pairs() {
        for (lead, trail) in [(one, other), (other, one)] {
            let message = conflict_message(run_pair(lead, trail, false)?);
            assert!(message.contains("/a"), "{name}: {message}");
        }
    }
    Ok(())
}

#[test]
fn a_join_orders_each_claims_gap_pair() -> Result<(), VfsError> {
    for (name, one, other) in racing_pairs() {
        for (lead, trail) in [(one, other), (other, one)] {
            if let Err(err) = run_pair(lead, trail, true)? {
                panic!("{name}, joined: {err:?}");
            }
        }
    }
    Ok(())
}

#[test]
fn a_listing_does_not_conflict_with_a_recursive_remove_of_a_grandchild() -> Result<(), VfsError> {
    let list: ClaimStep = |access| list_claim(access, "/a");
    let remove: ClaimStep = |access| access.remove("/a/b/c", true).map(|_| ());
    for (lead, trail) in [(list, remove), (remove, list)] {
        run_pair(lead, trail, false)??;
    }
    Ok(())
}
