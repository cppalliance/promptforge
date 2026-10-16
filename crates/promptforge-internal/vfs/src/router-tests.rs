use std::sync::{Arc, Mutex};

use super::{Mounts, Router};
use crate::error::VfsError;
use crate::handle::VfsRef;
use crate::memory::MemoryBackend;
use crate::test_support::{Call, TestFs, access, context, handle, path, test_origin};
use crate::traits::{Op, Vfs};

#[test]
fn the_longest_prefix_mount_wins_and_acquires_lazily() -> Result<(), VfsError> {
    let outer = TestFs::default();
    let inner = TestFs::default();
    let untouched = TestFs::default();
    let vfs = VfsRef::builder()
        .mount("/a", outer.clone())
        .mount("/a/b", inner.clone())
        .mount("/elsewhere", untouched.clone())
        .build();
    let access = access(&vfs)?;
    access.write("/a/b/f.txt", b"inner")?;
    access.write("/a/f.txt", b"outer")?;
    // Each backend keyed the file by its mount-relative path.
    assert!(inner.files().contains_key("/f.txt"));
    assert!(outer.files().contains_key("/f.txt"));
    assert_eq!(access.read("/a/b/f.txt")?, b"inner");
    assert_eq!(access.read("/a/f.txt")?, b"outer");
    // Lazy per-mount acquire: the untouched mount never opened one.
    let acquires = |fs: &TestFs| {
        fs.calls()
            .iter()
            .filter(|call| matches!(call, Call::Acquire(_)))
            .count()
    };
    assert_eq!(acquires(&inner), 1);
    assert_eq!(acquires(&outer), 1);
    assert_eq!(acquires(&untouched), 0);
    Ok(())
}

#[test]
fn a_longer_mount_shadows_the_same_prefix_of_a_shorter_one() -> Result<(), VfsError> {
    let base = TestFs::seeded(&[("/f.txt", "base-f"), ("/mnt/f.txt", "base-mnt")]);
    let shadow = TestFs::seeded(&[("/f.txt", "shadow-mnt")]);
    let vfs = VfsRef::builder()
        .mount("/", base.clone())
        .mount("/mnt", shadow.clone())
        .build();
    let access = access(&vfs)?;
    // The shadow mount owns everything under /mnt.
    assert_eq!(access.read("/mnt/f.txt")?, b"shadow-mnt");
    // The base still owns the rest of the namespace.
    assert_eq!(access.read("/f.txt")?, b"base-f");
    // The base's own /mnt/f.txt is unreachable through the handle.
    assert_eq!(
        base.files().get("/mnt/f.txt").map(Vec::as_slice),
        Some(b"base-mnt".as_slice())
    );
    Ok(())
}

#[test]
fn a_mounted_handle_applies_its_own_claims_under_the_callers_identity() -> Result<(), VfsError> {
    // Nesting: a base handle mounted under a child router.
    let base_storage = TestFs::default();
    let base = handle(base_storage.clone());
    let local = TestFs::default();
    let child = VfsRef::builder()
        .mount("/base", base.clone())
        .mount("/local", local.clone())
        .build();
    let writer = access(&child)?;
    writer.write("/base/f.txt", b"nested")?;
    // The child router stripped its mount prefix: the base backend
    // keyed the file at its own root.
    assert!(base_storage.files().contains_key("/f.txt"));
    // A second child identity conflicts on the same path: the claim
    // registered through the mounted handle is visible.
    let reader = access(&child)?;
    match reader.read("/base/f.txt") {
        Err(VfsError::Conflict { .. }) => {}
        other => panic!("expected a conflict, got {other:?}"),
    }
    // The local mount routes to its own backend.
    writer.write("/local/g.txt", b"local")?;
    assert!(local.files().contains_key("/g.txt"));
    Ok(())
}

#[test]
fn writes_to_a_read_only_mount_are_denied_without_partial_application() -> Result<(), VfsError> {
    let ro = TestFs::seeded(&[("/a.txt", "keep")]).with_read_only();
    let rw = TestFs::default();
    let vfs = VfsRef::builder()
        .mount("/", rw.clone())
        .mount("/ro", ro.clone())
        .build();
    let access = access(&vfs)?;
    // Reads are not gated.
    assert_eq!(access.read("/ro/a.txt")?, b"keep");
    // A write is refused as read-only, naming the path.
    assert_eq!(
        access.write("/ro/new.txt", b"x"),
        Err(VfsError::ReadOnly {
            path: "/ro/new.txt".to_owned(),
        })
    );
    assert!(!ro.files().contains_key("/new.txt"));
    // A rename wholly inside the mount is denied before the source
    // is touched.
    assert_eq!(
        access.rename("/ro/a.txt", "/ro/b.txt"),
        Err(VfsError::ReadOnly {
            path: "/ro/a.txt".to_owned(),
        })
    );
    assert_eq!(access.read("/ro/a.txt")?, b"keep");
    assert!(!ro.files().contains_key("/b.txt"));
    // A copy whose destination is read-only is denied; the source
    // is untouched.
    access.write("/x.txt", b"data")?;
    assert_eq!(
        access.copy("/x.txt", "/ro/x.txt"),
        Err(VfsError::ReadOnly {
            path: "/ro/x.txt".to_owned(),
        })
    );
    assert!(!ro.files().contains_key("/x.txt"));
    assert_eq!(access.read("/x.txt")?, b"data");
    Ok(())
}

#[test]
fn a_two_path_operation_across_mounts_is_refused_without_touching_either() -> Result<(), VfsError> {
    let left = TestFs::seeded(&[("/a.txt", "left")]);
    let right = TestFs::default();
    let vfs = VfsRef::builder()
        .mount("/left", left.clone())
        .mount("/right", right.clone())
        .build();
    let access = access(&vfs)?;
    for (op, result) in [
        ("rename", access.rename("/left/a.txt", "/right/a.txt")),
        ("copy", access.copy("/left/a.txt", "/right/a.txt")),
    ] {
        match result {
            Err(VfsError::CrossMount { path, detail }) => {
                assert_eq!(path, "/left/a.txt", "{op}");
                assert!(detail.starts_with(op), "{op}: {detail}");
            }
            other => panic!("expected a cross-mount refusal from {op}, got {other:?}"),
        }
    }
    assert_eq!(left.files().len(), 1);
    assert!(right.files().is_empty());
    Ok(())
}

#[test]
fn closing_an_access_reports_a_mount_release_failure_that_dropping_swallows() -> Result<(), VfsError>
{
    let refusal = || VfsError::Backend {
        message: "the release fails".to_owned(),
    };
    let mount = TestFs::default().on_release(move |_| Err(refusal()));
    let vfs = VfsRef::builder().mount("/m", mount.clone()).build();
    let closed = access(&vfs)?;
    let id = crate::detail::access_id(&closed);
    closed.write("/m/f.txt", b"x")?;
    assert_eq!(closed.close(), Err(refusal()));
    assert!(
        mount.calls().contains(&Call::Release(id)),
        "the mount was released"
    );
    // An access that never touched the mount has nothing to release.
    access(&vfs)?.close()?;
    // Dropping one that did releases once and swallows the failure.
    let dropped = access(&vfs)?;
    let id = crate::detail::access_id(&dropped);
    dropped.write("/m/g.txt", b"x")?;
    drop(dropped);
    let releases = mount
        .calls()
        .iter()
        .filter(|call| **call == Call::Release(id))
        .count();
    assert_eq!(releases, 1);
    Ok(())
}

/// The failure every release of a [`failing_release`] mount reports.
fn release_refusal() -> VfsError {
    VfsError::Backend {
        message: "the release fails".to_owned(),
    }
}

/// A mount whose every release fails with [`release_refusal`].
fn failing_release() -> TestFs {
    TestFs::default().on_release(|_| Err(release_refusal()))
}

#[test]
fn closing_an_access_reports_a_release_failure_inside_a_mounted_handle() -> Result<(), VfsError> {
    // The failing mount sits inside a handle that is itself mounted, so
    // its failure reaches the outer close only through the mounted
    // handle's session.
    let inner = VfsRef::builder().mount("/", failing_release()).build();
    let vfs = VfsRef::builder().mount("/m", inner).build();
    let closed = access(&vfs)?;
    closed.write("/m/f.txt", b"x")?;
    assert_eq!(closed.close(), Err(release_refusal()));
    Ok(())
}

/// Writes one file through a store view of a handle whose store is
/// `store`, then closes the view.
fn write_and_close_a_store_view(store: impl Vfs + 'static) -> Result<(), VfsError> {
    let vfs = VfsRef::builder().store("/s", store).build();
    let view = vfs.acquire_store(test_origin())?;
    view.write("f.txt", b"x")?;
    view.close()
}

#[test]
fn closing_a_store_view_reports_a_store_release_failure() {
    // The store's own release fails, or the release of a mount inside a
    // handle that serves as the store does.
    assert_eq!(
        write_and_close_a_store_view(failing_release()),
        Err(release_refusal())
    );
    let store = VfsRef::builder().mount("/", failing_release()).build();
    assert_eq!(write_and_close_a_store_view(store), Err(release_refusal()));
}

#[test]
fn traversal_that_escapes_the_namespace_root_is_rejected() -> Result<(), VfsError> {
    let vfs = VfsRef::builder().mount("/mnt", TestFs::default()).build();
    let access = access(&vfs)?;
    assert!(matches!(
        access.read("/mnt/../../etc/passwd"),
        Err(VfsError::InvalidPath { .. })
    ));
    assert!(matches!(
        access.glob("/mnt/../../*"),
        Err(VfsError::InvalidPath { .. })
    ));
    Ok(())
}

#[test]
fn a_path_that_climbs_out_of_its_mount_is_not_served_by_that_mount() -> Result<(), VfsError> {
    // No root mount: the only storage lives at /mnt.
    let storage = TestFs::seeded(&[("/f.txt", "inside")]);
    let vfs = VfsRef::builder().mount("/mnt", storage.clone()).build();
    let access = access(&vfs)?;
    // Dot segments within the mount resolve within the mount: the
    // backend sees the clean mount-relative path.
    assert_eq!(access.read("/mnt/sub/../f.txt")?, b"inside");
    assert!(
        storage
            .calls()
            .contains(&Call::Op(Op::Read, "/f.txt".to_owned()))
    );
    // A path that climbs out of the mount re-roots absolutely: it
    // canonicalizes to /secret.txt, no mount serves it, and the
    // mount's backend never sees the traversal spelling. The error's
    // path field names that unrouted path, not a routing sentence.
    match access.read("/mnt/../secret.txt") {
        Err(VfsError::NotFound { path }) => {
            assert_eq!(path, "/secret.txt");
        }
        other => panic!("expected no serving mount, got {other:?}"),
    }
    assert!(
        storage
            .calls()
            .iter()
            .all(|call| !matches!(call, Call::Op(_, path) if path.contains(".."))),
        "the backend never sees a traversal: {:?}",
        storage.calls()
    );
    Ok(())
}

#[test]
fn glob_routes_to_the_serving_mount_and_restores_the_prefix() -> Result<(), VfsError> {
    let inner = TestFs::seeded(&[("/x.txt", "x")]);
    let vfs = VfsRef::builder()
        .mount("/a", TestFs::default())
        .mount("/a/b", inner.clone())
        .build();
    let access = access(&vfs)?;
    let matches = access.glob("/a/b/*.txt")?;
    assert_eq!(matches, vec!["/a/b/x.txt".to_owned()]);
    Ok(())
}

#[test]
fn a_directory_only_glob_keeps_its_flag_across_a_mounted_handle() -> Result<(), VfsError> {
    // The overlay's base is a mounted handle: the dirs-only flag must
    // survive the mount forward, or the base's capability re-splits
    // the already-stripped pattern as files-only and the directories
    // vanish from the result.
    let base = VfsRef::builder().mount("/x", MemoryBackend::new()).build();
    let overlay = base.overlay("/overlay", TestFs::default());
    let access = access(&overlay)?;
    access.write("/x/d/a.txt", b"x")?;
    access.write("/x/d/sub/b.txt", b"x")?;
    assert_eq!(access.glob("x/d/*/")?, vec!["x/d/sub".to_owned()]);
    assert_eq!(access.glob("x/d/*")?, vec!["x/d/a.txt".to_owned()]);
    Ok(())
}

#[test]
fn removing_an_absent_path_through_a_mounted_handle_reports_not_found() -> Result<(), VfsError> {
    // The mount-forward chain at the backend level: the router's
    // session resolves the mount, and the mounted handle's forward
    // maps the capability's Ok(false) back onto the backend trait's
    // NotFound spelling.
    let base = handle(TestFs::default());
    let mut mounts = Mounts::new();
    mounts.insert(
        path("/base")?.to_buf(),
        Arc::new(Mutex::new(Box::new(base.clone()))),
    );
    let mut router = Router::new(mounts);
    let mut session = router.acquire(&context())?;
    assert!(matches!(
        session.remove(&path("/base/missing.txt")?, false),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn one_handle_serves_several_mounts_and_an_overlay_simultaneously() -> Result<(), VfsError> {
    let store = TestFs::default();
    let scratch = TestFs::default();
    let extra = TestFs::default();
    let base = VfsRef::builder()
        .mount("/store", store.clone())
        .mount("/scratch", scratch.clone())
        .build();
    let overlay = base.overlay("/overlay", extra.clone());

    let writer = access(&overlay)?;
    writer.write("/store/doc.md", b"store")?;
    writer.write("/scratch/tmp.txt", b"scratch")?;
    writer.write("/overlay/x.txt", b"overlay")?;
    // Dropping releases the writer's claims into the shared table.
    drop(writer);

    // The base handle serves its own mounts from the same storage.
    let reader = access(&base)?;
    assert_eq!(reader.read("/store/doc.md")?, b"store");
    assert_eq!(reader.read("/scratch/tmp.txt")?, b"scratch");
    // The overlay mount exists only in the overlay's view.
    assert!(matches!(
        reader.read("/overlay/x.txt"),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn an_overlay_shares_the_bases_claims_table() -> Result<(), VfsError> {
    let base = VfsRef::builder().mount("/store", TestFs::default()).build();
    let overlay = base.overlay("/overlay", TestFs::default());
    let first = access(&base)?;
    first.write("/store/shared.txt", b"1")?;
    // A write claim registered through the base conflicts with a
    // write attempted through the overlay: one claims table.
    let second = access(&overlay)?;
    match second.write("/store/shared.txt", b"2") {
        Err(VfsError::Conflict { detail, .. }) => {
            assert!(detail.contains("/store/shared.txt"), "{detail}");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    Ok(())
}

#[test]
#[should_panic(expected = "invalid mount prefix")]
fn the_builder_rejects_a_relative_mount_prefix() {
    let _ = VfsRef::builder().mount("relative", TestFs::default());
}

#[test]
#[should_panic(expected = "invalid store root")]
fn the_builder_rejects_a_relative_store_root() {
    let _ = VfsRef::builder().store("relative", TestFs::default());
}

#[test]
#[should_panic(expected = "a mount already sits at")]
fn the_builder_rejects_a_store_root_that_already_has_a_mount() {
    let _ = VfsRef::builder()
        .mount("/s", TestFs::default())
        .store("/s", TestFs::default());
}
