//! Tests for refusals: a backend that refuses acquisition, at acquire,
//! through a mount, and at spawn, and an identity whose tool call has
//! ended while its scope stays open.

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::MemoryBackend;
use crate::detail::{end_access, scope_handle};

/// The error a refusing backend's acquire returns: the trait's contract
/// allows refusal, so the handle must surface it as an error rather
/// than panic.
fn refusal() -> VfsError {
    VfsError::Backend {
        message: "the backend refuses acquisition".to_owned(),
    }
}

#[test]
fn a_backend_refusal_fails_acquire_with_an_error_instead_of_panicking() {
    let vfs = VfsRef::new(TestFs::default().on_acquire(|_| Err(refusal())));
    match access(&vfs) {
        Err(VfsError::Backend { message }) => {
            assert_eq!(message, "the backend refuses acquisition");
        }
        other => panic!("expected a backend refusal, got {other:?}"),
    }
}

#[test]
fn a_mounted_handle_whose_backend_refuses_releases_the_identity_it_joined() -> Result<(), VfsError>
{
    let vfs = VfsRef::builder()
        .mount("/", TestFs::default())
        .mount(
            "/m",
            VfsRef::new(TestFs::default().on_acquire(|_| Err(refusal()))),
        )
        .build();
    let caller = access(&vfs)?;
    caller.write("/a.txt", b"1")?;
    match caller.read("/m/x.txt") {
        Err(VfsError::Backend { message }) => {
            assert_eq!(message, "the backend refuses acquisition");
        }
        other => panic!("expected a backend refusal, got {other:?}"),
    }
    let scope = Arc::clone(&caller.scope);
    drop(caller);
    // The refused mount's join was released, so the scope ends with
    // the caller's last access and its claims stop conflicting.
    assert_eq!(scope.live.load(Ordering::Acquire), 0);
    let fresh = access(&vfs)?;
    fresh.write("/a.txt", b"2")?;
    Ok(())
}

#[test]
fn a_backend_refusal_fails_spawn_and_releases_the_refused_child() -> Result<(), VfsError> {
    // The backend refuses exactly its second acquisition: the spawn
    // is the second.
    let vended = AtomicUsize::new(0);
    let vfs = VfsRef::new(TestFs::default().on_acquire(move |_| {
        if vended.fetch_add(1, Ordering::Relaxed) == 1 {
            return Err(refusal());
        }
        Ok(())
    }));
    let parent = access(&vfs)?;
    parent.write("/f.txt", b"1")?;
    match parent.spawn(test_origin()) {
        Err(VfsError::Backend { message }) => {
            assert_eq!(message, "the backend refuses acquisition");
        }
        other => panic!("expected a backend refusal, got {other:?}"),
    }
    // The refused child's reference was released, so only the
    // parent keeps the scope live, and the parent's write still
    // conflicts with another live scope's.
    assert_eq!(parent.scope.live.load(Ordering::Acquire), 1);
    let other = access(&vfs)?;
    let message = conflict_message(other.write("/f.txt", b"2"));
    assert!(message.contains("/f.txt"), "{message}");
    Ok(())
}

#[test]
fn an_ended_identity_refuses_operations_spawns_and_views_while_the_scope_stays_open()
-> Result<(), VfsError> {
    fn refused<T: std::fmt::Debug>(result: Result<T, VfsError>, expected: &str) {
        match result {
            Err(VfsError::ScopeEnded { path }) => assert_eq!(path, expected),
            other => panic!("expected an ended-call refusal on {expected}, got {other:?}"),
        }
    }

    let vfs = VfsRef::builder()
        .mount("/", MemoryBackend::new())
        .store("/my/store", MemoryBackend::new())
        .build();
    let owner = access(&vfs)?;
    let call = owner.spawn(test_origin())?;
    call.write("/a.txt", b"call")?;
    let view = call.store_view()?;
    end_access(&scope_handle(&owner), Some(owner.id), call.id);
    refused(call.read("/a.txt"), "/a.txt");
    refused(call.write("/b.txt", b"late"), "/b.txt");
    refused(view.write("b.md", b"late"), "b.md");
    refused(call.spawn(test_origin()), "/");
    refused(call.store_view(), "/");
    // The scope stays open: the end joined the call's write into the
    // owner, and the owner still spawns and derives views.
    assert!(!owner.scope.ended());
    assert_eq!(owner.read("/a.txt")?, b"call");
    assert!(!owner.exists("/b.txt")?);
    assert!(!owner.exists("/my/store/b.md")?);
    owner.spawn(test_origin())?.write("/c.txt", b"arm")?;
    owner.store_view()?.write("c.md", b"view")?;
    Ok(())
}
