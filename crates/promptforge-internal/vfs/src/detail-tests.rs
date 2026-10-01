use super::{access_spawn, end_scope, probe_store, scope_handle, store_view};
use crate::{
    Access, AcquireContext, ExecId, MemoryBackend, Origin, PathReason, Vfs, VfsAccess, VfsError,
    VfsRef,
};

/// A base at `/` beside a store declared at `/my/store`: the shape
/// most store-view tests start from.
fn stock() -> VfsRef {
    VfsRef::builder()
        .mount("/", MemoryBackend::new())
        .store("/my/store", MemoryBackend::new())
        .build()
}

/// The chain's access and its store view, in one scope.
fn chain(vfs: &VfsRef) -> (Access, Access) {
    let access = vfs
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    let view = store_view(&access).expect("the handle declares a store");
    (access, view)
}

#[test]
fn an_ended_scope_refuses_spawns_views_and_arm_operations_and_frees_its_claims()
-> Result<(), VfsError> {
    let vfs = stock();
    let (access, view) = chain(&vfs);
    view.write("a.txt", b"run")?;
    let arm = access_spawn(&access, Origin::new("arm"))?;
    end_scope(&scope_handle(&access));
    let ended = |result: Result<Access, VfsError>| match result {
        Err(VfsError::PermissionDenied { reason, .. }) => {
            assert!(reason.contains("has ended"), "{reason}");
        }
        other => panic!("expected an ended-run refusal, got {other:?}"),
    };
    ended(access_spawn(&access, Origin::new("late arm")));
    ended(store_view(&access));
    ended(store_view(&arm));
    match arm.read("/my/store/a.txt") {
        Err(VfsError::PermissionDenied { path, .. }) => assert_eq!(path, "/my/store/a.txt"),
        other => panic!("expected an ended-run refusal, got {other:?}"),
    }
    // Another scope writes the path at once: the ended scope's claim
    // no longer conflicts, though its accesses are still held.
    let (_other, other_view) = chain(&vfs);
    other_view.write("a.txt", b"next")?;
    assert_eq!(other_view.read("a.txt")?, b"next");
    Ok(())
}

#[test]
fn each_strict_path_rule_is_reported_in_check_order() {
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    // The rules run in this order, and the first one broken is the
    // one reported, with the path exactly as supplied.
    for (path, reason) in [
        ("", PathReason::Empty),
        ("/absolute.txt", PathReason::Absolute),
        ("a\u{0}b.txt", PathReason::Control),
        ("a\\b.txt", PathReason::Backslash),
        ("a//b.txt", PathReason::EmptySegment),
        ("../escape.txt", PathReason::Traversal),
        ("a/./b.txt", PathReason::Traversal),
        ("trailing.", PathReason::UnsafeSuffix),
        ("trailing ", PathReason::UnsafeSuffix),
        ("CON", PathReason::ReservedName),
        ("dir/nul.txt", PathReason::ReservedName),
        ("com1", PathReason::ReservedName),
        ("LPT9.log", PathReason::ReservedName),
    ] {
        match view.read(path) {
            Err(VfsError::InvalidPath {
                path: reported,
                reason: got,
            }) => {
                assert_eq!(reported, path, "reports the path as supplied");
                assert_eq!(got, reason, "{path:?}");
            }
            other => panic!("expected InvalidPath for {path:?}, got {other:?}"),
        }
    }
}

#[test]
fn the_path_length_ceiling_is_1024_bytes() -> Result<(), VfsError> {
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    // A path at the exact ceiling is accepted; one byte over is
    // rejected.
    let maximum = "a".repeat(1024);
    view.write(&maximum, b"at the ceiling")?;
    assert_eq!(view.read(&maximum)?, b"at the ceiling");
    let too_long = "a".repeat(1025);
    match view.read(&too_long) {
        Err(VfsError::InvalidPath {
            path,
            reason: PathReason::TooLong,
        }) => assert_eq!(path, too_long),
        other => panic!("expected TooLong, got {other:?}"),
    }
    // Names that merely contain a device substring are allowed.
    view.write("console.txt", b"ok")?;
    view.write("com10.txt", b"ok")?;
    Ok(())
}

#[test]
fn errors_report_paths_in_the_callers_relative_form() -> Result<(), VfsError> {
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    // A backend error reports the logical path, not the canonical
    // one under the store root.
    match view.read("absent.txt") {
        Err(VfsError::NotFound { path }) => assert_eq!(path, "absent.txt"),
        other => panic!("expected NotFound, got {other:?}"),
    }
    view.write("f.txt", b"one")?;
    // A second chain's write races, and the diagnosis names the
    // logical path, never the canonical one.
    let other_access = vfs
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    let other = store_view(&other_access).expect("the handle declares a store");
    match other.write("f.txt", b"two") {
        Err(VfsError::Conflict { path, detail }) => {
            assert_eq!(path, "f.txt");
            assert!(detail.contains("f.txt"), "{detail}");
            assert!(!detail.contains("/my/store"), "{detail}");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    // The raced write never landed.
    assert_eq!(view.read("f.txt")?, b"one");
    Ok(())
}

#[test]
fn a_backend_error_under_a_path_that_repeats_the_store_root_keeps_the_whole_path() {
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    // The backend names the mount-relative `/my/store/x.md`, which
    // only looks like a path under the store root.
    match view.read("my/store/x.md") {
        Err(VfsError::NotFound { path }) => assert_eq!(path, "my/store/x.md"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn a_two_path_backend_error_keeps_a_destination_that_repeats_the_store_root() -> Result<(), VfsError>
{
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    view.write("x.md", b"source")?;
    view.mkdir("my/store/x.md", true)?;
    // The backend names the destination's mount-relative
    // `/my/store/x.md`, which is also the source's canonical path.
    for (op, result) in [
        ("copy", view.copy("x.md", "my/store/x.md")),
        ("rename", view.rename("x.md", "my/store/x.md")),
    ] {
        match result {
            Err(VfsError::IsADirectory { path }) => assert_eq!(path, "my/store/x.md", "{op}"),
            other => panic!("expected IsADirectory from {op}, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_read_only_store_reports_its_denial_in_the_callers_relative_form() {
    let vfs = VfsRef::builder()
        .store("/my/store", ReadOnly(MemoryBackend::new()))
        .build();
    let (_access, view) = chain(&vfs);
    // The router refuses before the backend, naming the canonical
    // path; both spellings come back in the logical form.
    for (path, expected) in [("x.md", "x.md"), ("my/store/x.md", "my/store/x.md")] {
        match view.write(path, b"denied") {
            Err(VfsError::PermissionDenied { path, reason }) => {
                assert_eq!(path, expected);
                assert!(
                    reason.contains(&format!("so {expected} cannot")),
                    "{reason}"
                );
            }
            other => panic!("expected a read-only denial for {path:?}, got {other:?}"),
        }
    }
}

#[test]
fn the_store_probe_acquires_the_store_backend() {
    let vfs = VfsRef::builder().store("/my/store", Refusing).build();
    let access = vfs
        .acquire(Origin::new("store view test"))
        .expect("the router acquires lazily");
    // Deriving the view alone never reaches the store backend.
    assert!(store_view(&access).is_ok());
    match probe_store(&access) {
        Err(VfsError::Backend { message }) => assert!(message.contains("refuses"), "{message}"),
        other => panic!("expected the backend's refusal, got {other:?}"),
    }
}

#[test]
fn the_store_probe_treats_a_missing_store_root_as_success() -> Result<(), VfsError> {
    // A wrapped handle that mounts nothing at `/` reports the store
    // root missing.
    let inner = VfsRef::builder()
        .mount("/data", MemoryBackend::new())
        .build();
    let vfs = VfsRef::builder().store("/", inner).build();
    let access = vfs.acquire(Origin::new("store view test"))?;
    assert!(matches!(access.stat("/"), Err(VfsError::NotFound { .. })));
    probe_store(&access)
}

/// A backend whose sessions always refuse to open.
struct Refusing;

impl Vfs for Refusing {
    fn acquire(&mut self, _cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        Err(VfsError::Backend {
            message: "the store backend refuses every session".to_owned(),
        })
    }

    fn release(&mut self, _id: ExecId) -> Result<(), VfsError> {
        Ok(())
    }
}

/// A backend that forwards to `0` but reports itself read-only.
struct ReadOnly(MemoryBackend);

impl Vfs for ReadOnly {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        self.0.acquire(cx)
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.0.release(id)
    }

    fn read_only(&self) -> bool {
        true
    }
}

#[test]
fn one_chain_never_conflicts_with_itself_through_its_view() -> Result<(), VfsError> {
    let vfs = stock();
    let (access, view) = chain(&vfs);
    view.write("a.txt", b"one")?;
    view.append("a.txt", b"two")?;
    assert_eq!(view.read("a.txt")?, b"onetwo");
    // A second view over the same chain shares the identity and the
    // scope, so its operations never race the first view's.
    let also = store_view(&access).expect("the handle declares a store");
    assert_eq!(also.read("a.txt")?, b"onetwo");
    also.write("b.txt", b"three")?;
    // The chain's plain capability reads the same storage.
    assert_eq!(access.read("/my/store/b.txt")?, b"three");
    Ok(())
}

#[test]
fn a_store_at_the_root_cannot_reach_a_mount_beneath_it() -> Result<(), VfsError> {
    let store = MemoryBackend::new();
    let vfs = VfsRef::builder()
        .store("/", store.clone())
        .mount("/host", MemoryBackend::new())
        .build();
    {
        let (_access, view) = chain(&vfs);
        view.write("host/secret.txt", b"stored")?;
    }
    // The write landed in the store mount's own storage...
    let probe = VfsRef::new(store.clone());
    let probe_access = probe
        .acquire(Origin::new("store view test"))
        .expect("the probe backend acquires");
    assert!(probe_access.exists("/host/secret.txt")?);
    // ...and the `/host` directory mounted beneath the store never saw it.
    let outer = vfs
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    assert!(!outer.exists("/host/secret.txt")?);
    Ok(())
}

#[test]
fn vfsref_default_is_a_memory_store_at_the_root() -> Result<(), VfsError> {
    let vfs = VfsRef::default();
    let (access, view) = chain(&vfs);
    view.write("notes.md", b"# draft")?;
    assert_eq!(view.read("notes.md")?, b"# draft");
    // The store is the whole namespace: the plain capability and
    // the view address one storage.
    assert_eq!(access.read("/notes.md")?, b"# draft");
    Ok(())
}

#[test]
fn a_base_at_the_root_serves_beside_the_declared_store() -> Result<(), VfsError> {
    let vfs = stock();
    let (access, view) = chain(&vfs);
    view.write("a.txt", b"store")?;
    view.write("b.md", b"markdown")?;
    access.write("/draft.md", b"base")?;
    // The view sees the store; the chain sees both mounts.
    assert_eq!(view.read("a.txt")?, b"store");
    assert_eq!(access.read("/draft.md")?, b"base");
    assert_eq!(access.read("/my/store/a.txt")?, b"store");
    // An absolute path is outside the store's logical namespace.
    match view.read("/draft.md") {
        Err(VfsError::InvalidPath {
            reason: PathReason::Absolute,
            ..
        }) => {}
        other => panic!("expected an absolute-path refusal, got {other:?}"),
    }
    // Glob patterns and their results stay in the logical form.
    assert_eq!(view.glob("*.txt")?, vec!["a.txt".to_owned()]);
    Ok(())
}

#[test]
fn the_store_views_glob_runs_the_strict_rules_on_the_pattern() -> Result<(), VfsError> {
    let vfs = stock();
    let (_access, view) = chain(&vfs);
    view.write("a.txt", b"")?;
    // An absolute pattern would canonicalize namespace-absolute
    // and register its claim outside the store mount; a traversal
    // pattern is refused like any other store path.
    for (pattern, reason) in [
        ("/etc/*", PathReason::Absolute),
        ("../*.txt", PathReason::Traversal),
        ("a/./b", PathReason::Traversal),
    ] {
        match view.glob(pattern) {
            Err(VfsError::InvalidPath {
                path: reported,
                reason: got,
            }) => {
                assert_eq!(reported, pattern, "reports the pattern as supplied");
                assert_eq!(got, reason, "{pattern:?}");
            }
            other => {
                panic!("expected an invalid-pattern refusal for {pattern:?}, got {other:?}");
            }
        }
    }
    // Well-formed patterns - whole-segment `**` included - match
    // inside the mount.
    assert_eq!(view.glob("*.txt")?, vec!["a.txt".to_owned()]);
    assert_eq!(view.glob("**")?, vec!["a.txt".to_owned()]);
    Ok(())
}

#[test]
fn an_overlay_inherits_its_bases_store() -> Result<(), VfsError> {
    let base = stock();
    let overlay = base.overlay("/overlay", MemoryBackend::new());
    {
        let (access, view) = chain(&overlay);
        view.write("notes.md", b"overlay")?;
        access.write("/overlay/x.txt", b"extra")?;
        assert_eq!(access.read("/overlay/x.txt")?, b"extra");
    }
    // The base handle reads the store write: one store, shared
    // storage, served through either handle.
    let base_access = base
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    assert_eq!(base_access.read("/my/store/notes.md")?, b"overlay");
    Ok(())
}

#[test]
fn the_outermost_declaration_counts() -> Result<(), VfsError> {
    // A handle mounted as a backend keeps its declaration to
    // itself: the mounting handle declares no store, so no view.
    let inner = VfsRef::builder().store("/s", MemoryBackend::new()).build();
    let outer = VfsRef::builder().mount("/inner", inner).build();
    let access = outer
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    match store_view(&access) {
        Err(VfsError::Unsupported { detail, .. }) => {
            assert!(detail.contains("no store"), "{detail}");
        }
        other => panic!("expected no declared store, got {other:?}"),
    }
    drop(access);

    // The outer builder's own declaration is the one the view uses.
    let inner = VfsRef::builder().store("/s", MemoryBackend::new()).build();
    let outer = VfsRef::builder()
        .mount("/inner", inner)
        .store("/outer-store", MemoryBackend::new())
        .build();
    let access = outer
        .acquire(Origin::new("store view test"))
        .expect("the stock backend acquires");
    let view = store_view(&access).expect("the outer handle declares a store");
    view.write("a.txt", b"outer")?;
    drop(view);
    assert!(access.exists("/outer-store/a.txt")?);
    assert!(!access.exists("/inner/s/a.txt")?);
    Ok(())
}

#[test]
fn acquire_store_roots_logical_paths_at_the_declared_store() -> Result<(), VfsError> {
    let vfs = stock();
    {
        let view = vfs.acquire_store(Origin::new("acquire_store test"))?;
        view.write("paper.md", b"seeded")?;
        assert_eq!(view.read("paper.md")?, b"seeded");
    }
    // The write landed under the store root, beside the base at `/`.
    let access = vfs.acquire(Origin::new("acquire_store test"))?;
    assert_eq!(access.read("/my/store/paper.md")?, b"seeded");
    assert!(!access.exists("/paper.md")?);
    Ok(())
}

#[test]
fn acquire_store_applies_the_strict_path_rules() -> Result<(), VfsError> {
    let vfs = stock();
    let view = vfs.acquire_store(Origin::new("acquire_store test"))?;
    match view.write("../escape.md", b"out") {
        Err(VfsError::InvalidPath {
            path,
            reason: PathReason::Traversal,
        }) => assert_eq!(path, "../escape.md"),
        other => panic!("expected a traversal refusal, got {other:?}"),
    }
    Ok(())
}

#[test]
fn acquire_store_on_a_handle_without_a_store_is_unsupported() {
    let vfs = VfsRef::builder().mount("/", MemoryBackend::new()).build();
    match vfs.acquire_store(Origin::new("acquire_store test")) {
        Err(VfsError::Unsupported { detail, .. }) => {
            assert!(detail.contains("no store"), "{detail}");
        }
        other => panic!("expected no declared store, got {other:?}"),
    }
}

#[test]
fn acquire_store_is_a_scope_of_its_own() -> Result<(), VfsError> {
    let vfs = stock();
    let (_access, run_view) = chain(&vfs);
    run_view.write("claimed.md", b"run")?;
    // The Harness's view is a second scope: the live run's claim conflicts.
    let host = vfs.acquire_store(Origin::new("acquire_store test"))?;
    assert!(matches!(
        host.write("claimed.md", b"host"),
        Err(VfsError::Conflict { .. })
    ));
    Ok(())
}
