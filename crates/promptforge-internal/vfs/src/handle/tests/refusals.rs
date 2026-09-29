//! Tests for a backend that refuses acquisition, at acquire, through a
//! mount, and at spawn.

use super::*;

/// A backend that refuses every acquisition: the trait's contract
/// allows refusal, so the handle must surface it as an error rather
/// than panic.
struct RefusingFs;

impl Vfs for RefusingFs {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        let _ = cx;
        Err(VfsError::Backend {
            message: "the backend refuses acquisition".to_owned(),
        })
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        let _ = id;
        Ok(())
    }
}

#[test]
fn a_backend_refusal_fails_acquire_with_an_error_instead_of_panicking() {
    let vfs = VfsRef::new(RefusingFs);
    match vfs.acquire(test_origin()) {
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
        .mount("/", StubFs::default())
        .mount("/m", VfsRef::new(RefusingFs))
        .build();
    let access = vfs.acquire(test_origin())?;
    access.write("/a.txt", b"1")?;
    match access.read("/m/x.txt") {
        Err(VfsError::Backend { message }) => {
            assert_eq!(message, "the backend refuses acquisition");
        }
        other => panic!("expected a backend refusal, got {other:?}"),
    }
    let scope = Arc::clone(&access.scope);
    drop(access);
    // The refused mount's join was released, so the scope ends with
    // the caller's last access and its claims stop conflicting.
    assert_eq!(scope.live.load(Ordering::Acquire), 0);
    let fresh = vfs.acquire(test_origin())?;
    fresh.write("/a.txt", b"2")?;
    Ok(())
}

#[test]
fn a_backend_refusal_fails_spawn_and_releases_the_refused_child() -> Result<(), VfsError> {
    /// A backend that refuses exactly its second acquisition: the
    /// spawn is the second.
    struct RefuseSecond {
        vended: Arc<Mutex<usize>>,
    }

    impl Vfs for RefuseSecond {
        fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
            let _ = cx;
            let mut vended = self.vended.lock().unwrap_or_else(PoisonError::into_inner);
            *vended += 1;
            if *vended == 2 {
                return Err(VfsError::Backend {
                    message: "the backend refuses acquisition".to_owned(),
                });
            }
            Ok(Box::new(StubAccess {
                files: Arc::new(Mutex::new(BTreeMap::new())),
            }))
        }

        fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
            let _ = id;
            Ok(())
        }
    }

    let vfs = VfsRef::new(RefuseSecond {
        vended: Arc::new(Mutex::new(0)),
    });
    let parent = vfs.acquire(test_origin())?;
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
    let other = vfs.acquire(test_origin())?;
    let message = conflict_message(other.write("/f.txt", b"2"));
    assert!(message.contains("/f.txt"), "{message}");
    Ok(())
}
