use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{Mounts, Router};
use crate::error::VfsError;
use crate::handle::{Scope, VfsRef};
use crate::memory::MemoryBackend;
use crate::observe::Origin;
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::{Entry, FileType, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

/// A recording in-memory stub. Files are keyed by the exact paths
/// the backend is handed, so tests observe prefix stripping
/// directly; every served path is recorded; the acquire count pins
/// lazy per-mount acquisition.
#[derive(Clone, Default)]
struct StubFs {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    seen: Arc<Mutex<Vec<String>>>,
    acquires: Arc<Mutex<usize>>,
    read_only: bool,
}

impl StubFs {
    fn seeded(files: &[(&str, &str)]) -> StubFs {
        let stub = StubFs::default();
        for (name, text) in files {
            stub.files()
                .insert((*name).to_owned(), text.as_bytes().to_vec());
        }
        stub
    }

    fn with_read_only(mut self) -> StubFs {
        self.read_only = true;
        self
    }

    fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn acquire_count(&self) -> usize {
        *self.acquires.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Vfs for StubFs {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        let _ = cx;
        *self.acquires.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Ok(Box::new(StubAccess {
            files: Arc::clone(&self.files),
            seen: Arc::clone(&self.seen),
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        let _ = id;
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.read_only
    }
}

struct StubAccess {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl StubAccess {
    fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, path: &VfsPath) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.to_string());
    }
}

impl VfsAccess for StubAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.record(path);
        self.files()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.record(path);
        self.files().insert(path.to_string(), contents.to_vec());
        Ok(())
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.record(path);
        self.files()
            .entry(path.to_string())
            .or_default()
            .extend_from_slice(contents);
        Ok(())
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let _ = recursive;
        self.record(path);
        self.files()
            .remove(path.as_str())
            .map(|_| ())
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.record(path);
        Ok(self.files().contains_key(path.as_str()))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        let prefix = pattern.split('*').next().unwrap_or(pattern);
        let mut matches: Vec<String> = self
            .files()
            .keys()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect();
        matches.sort();
        Ok(matches)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        Err(VfsError::Unsupported {
            path: path.to_string(),
            detail: "the stub does not list".into(),
        })
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        // Every stored key is a file; the stub holds no directories.
        let bytes = self
            .files()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        Ok(Stat {
            file_type: FileType::File,
            size: bytes.len() as u64,
            mode: None,
            modified: None,
            created: None,
        })
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let _ = (path, recursive);
        Ok(())
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.record(from);
        self.record(to);
        let bytes = self
            .files()
            .remove(from.as_str())
            .ok_or_else(|| VfsError::NotFound {
                path: from.to_string(),
            })?;
        self.files().insert(to.to_string(), bytes);
        Ok(())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.record(from);
        self.record(to);
        let bytes = self
            .files()
            .get(from.as_str())
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: from.to_string(),
            })?;
        self.files().insert(to.to_string(), bytes);
        Ok(())
    }
}

/// The routing tests ignore origins, so they acquire under one
/// blanket label.
fn test_origin() -> Origin {
    Origin::new("router test")
}

#[test]
fn the_longest_prefix_mount_wins_and_acquires_lazily() -> Result<(), VfsError> {
    let outer = StubFs::default();
    let inner = StubFs::default();
    let untouched = StubFs::default();
    let vfs = VfsRef::builder()
        .mount("/a", outer.clone())
        .mount("/a/b", inner.clone())
        .mount("/elsewhere", untouched.clone())
        .build();
    let access = vfs.acquire(test_origin())?;
    access.write("/a/b/f.txt", b"inner")?;
    access.write("/a/f.txt", b"outer")?;
    // Each backend keyed the file by its mount-relative path.
    assert!(inner.files().contains_key("/f.txt"));
    assert!(outer.files().contains_key("/f.txt"));
    assert_eq!(access.read("/a/b/f.txt")?, b"inner");
    assert_eq!(access.read("/a/f.txt")?, b"outer");
    // Lazy per-mount acquire: the untouched mount never opened one.
    assert_eq!(inner.acquire_count(), 1);
    assert_eq!(outer.acquire_count(), 1);
    assert_eq!(untouched.acquire_count(), 0);
    Ok(())
}

#[test]
fn a_longer_mount_shadows_the_same_prefix_of_a_shorter_one() -> Result<(), VfsError> {
    let base = StubFs::seeded(&[("/f.txt", "base-f"), ("/mnt/f.txt", "base-mnt")]);
    let shadow = StubFs::seeded(&[("/f.txt", "shadow-mnt")]);
    let vfs = VfsRef::builder()
        .mount("/", base.clone())
        .mount("/mnt", shadow.clone())
        .build();
    let access = vfs.acquire(test_origin())?;
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
    let base_storage = StubFs::default();
    let base = VfsRef::new(base_storage.clone());
    let local = StubFs::default();
    let child = VfsRef::builder()
        .mount("/base", base.clone())
        .mount("/local", local.clone())
        .build();
    let writer = child.acquire(test_origin())?;
    writer.write("/base/f.txt", b"nested")?;
    // The child router stripped its mount prefix: the base backend
    // keyed the file at its own root.
    assert!(base_storage.files().contains_key("/f.txt"));
    // A second child identity conflicts on the same path: the claim
    // registered through the mounted handle is visible.
    let reader = child.acquire(test_origin())?;
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
    let ro = StubFs::seeded(&[("/a.txt", "keep")]).with_read_only();
    let rw = StubFs::default();
    let vfs = VfsRef::builder()
        .mount("/", rw.clone())
        .mount("/ro", ro.clone())
        .build();
    let access = vfs.acquire(test_origin())?;
    // Reads are not gated.
    assert_eq!(access.read("/ro/a.txt")?, b"keep");
    // A write is denied with a clear read-only error.
    match access.write("/ro/new.txt", b"x") {
        Err(VfsError::PermissionDenied { reason, .. }) => {
            assert!(reason.contains("read-only"), "names the cause: {reason}");
        }
        other => panic!("expected a read-only denial, got {other:?}"),
    }
    assert!(!ro.files().contains_key("/new.txt"));
    // A rename wholly inside the mount is denied before the source
    // is touched.
    match access.rename("/ro/a.txt", "/ro/b.txt") {
        Err(VfsError::PermissionDenied { .. }) => {}
        other => panic!("expected a read-only denial, got {other:?}"),
    }
    assert_eq!(access.read("/ro/a.txt")?, b"keep");
    assert!(!ro.files().contains_key("/b.txt"));
    // A copy whose destination is read-only is denied; the source
    // is untouched.
    access.write("/x.txt", b"data")?;
    match access.copy("/x.txt", "/ro/x.txt") {
        Err(VfsError::PermissionDenied { .. }) => {}
        other => panic!("expected a read-only denial, got {other:?}"),
    }
    assert!(!ro.files().contains_key("/x.txt"));
    assert_eq!(access.read("/x.txt")?, b"data");
    Ok(())
}

#[test]
fn traversal_that_escapes_the_namespace_root_is_rejected() -> Result<(), VfsError> {
    let vfs = VfsRef::builder().mount("/mnt", StubFs::default()).build();
    let access = vfs.acquire(test_origin())?;
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
    let storage = StubFs::seeded(&[("/f.txt", "inside")]);
    let vfs = VfsRef::builder().mount("/mnt", storage.clone()).build();
    let access = vfs.acquire(test_origin())?;
    // Dot segments within the mount resolve within the mount: the
    // backend sees the clean mount-relative path.
    assert_eq!(access.read("/mnt/sub/../f.txt")?, b"inside");
    assert!(storage.seen().contains(&"/f.txt".to_owned()));
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
        storage.seen().iter().all(|path| !path.contains("..")),
        "the backend never sees a traversal: {:?}",
        storage.seen()
    );
    Ok(())
}

#[test]
fn glob_routes_to_the_serving_mount_and_restores_the_prefix() -> Result<(), VfsError> {
    let inner = StubFs::seeded(&[("/x.txt", "x")]);
    let vfs = VfsRef::builder()
        .mount("/a", StubFs::default())
        .mount("/a/b", inner.clone())
        .build();
    let access = vfs.acquire(test_origin())?;
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
    let overlay = base.overlay("/overlay", StubFs::default());
    let access = overlay.acquire(test_origin())?;
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
    let base = VfsRef::new(StubFs::default());
    let mut mounts = Mounts::new();
    mounts.insert(
        canonicalize_absolute("/base")?.to_buf(),
        Arc::new(Mutex::new(Box::new(base.clone()))),
    );
    let mut router = Router::new(mounts);
    let mut session = router.acquire(&AcquireContext::new(ExecId::vend(), Scope::start()))?;
    assert!(matches!(
        session.remove(&canonicalize_absolute("/base/missing.txt")?, false),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn one_handle_serves_several_mounts_and_an_overlay_simultaneously() -> Result<(), VfsError> {
    let store = StubFs::default();
    let scratch = StubFs::default();
    let extra = StubFs::default();
    let base = VfsRef::builder()
        .mount("/store", store.clone())
        .mount("/scratch", scratch.clone())
        .build();
    let overlay = base.overlay("/overlay", extra.clone());

    let writer = overlay.acquire(test_origin())?;
    writer.write("/store/doc.md", b"store")?;
    writer.write("/scratch/tmp.txt", b"scratch")?;
    writer.write("/overlay/x.txt", b"overlay")?;
    // Dropping releases the writer's claims into the shared table.
    drop(writer);

    // The base handle serves its own mounts from the same storage.
    let reader = base.acquire(test_origin())?;
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
    let base = VfsRef::builder().mount("/store", StubFs::default()).build();
    let overlay = base.overlay("/overlay", StubFs::default());
    let first = base.acquire(test_origin())?;
    first.write("/store/shared.txt", b"1")?;
    // A write claim registered through the base conflicts with a
    // write attempted through the overlay: one claims table.
    let second = overlay.acquire(test_origin())?;
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
    let _ = VfsRef::builder().mount("relative", StubFs::default());
}

#[test]
#[should_panic(expected = "invalid store root")]
fn the_builder_rejects_a_relative_store_root() {
    let _ = VfsRef::builder().store("relative", StubFs::default());
}

#[test]
#[should_panic(expected = "a mount already sits at")]
fn the_builder_rejects_a_store_root_that_already_has_a_mount() {
    let _ = VfsRef::builder()
        .mount("/s", StubFs::default())
        .store("/s", StubFs::default());
}
