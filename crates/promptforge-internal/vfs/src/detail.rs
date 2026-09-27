//! Operations on [`Access`] that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host: a host passes a run's capability through, and
//! the engine alone forks it for concurrent arms, joins the arms'
//! identities back on delivery, derives the store view for store
//! calls, and ends the run's scope when the run ends.

use std::fmt;
use std::sync::{Arc, Weak};

use crate::handle::Scope;
use crate::{Access, ExecId, Origin, VfsError};

/// An opaque handle on the scope an [`Access`] belongs to, which the
/// engine holds for the life of a run so [`end_scope`] can close it. It
/// never keeps the scope alive.
#[derive(Clone)]
pub struct ScopeHandle(Weak<Scope>);

impl fmt::Debug for ScopeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ScopeHandle").finish_non_exhaustive()
    }
}

/// The handle on `access`'s scope: the scope of every identity spawned
/// from it, however deep.
#[must_use]
pub fn scope_handle(access: &Access) -> ScopeHandle {
    ScopeHandle(Arc::downgrade(access.scope()))
}

/// Ends the scope behind `scope`: its claims stop conflicting at once,
/// and every access still held in it - a store view a host kept past the
/// run, a forwarded mount session - refuses its next operation, spawn,
/// or store view with [`VfsError::PermissionDenied`]. Idempotent, and a
/// no-op once every access in the scope has dropped.
pub fn end_scope(scope: &ScopeHandle) {
    if let Some(scope) = scope.0.upgrade() {
        scope.close();
    }
}

/// Derives the store view from a chain's [`Access`]: an ordinary
/// [`Access`] rooted at the handle's declared store root, whose
/// operations reach the store's own mount alone. The view keeps the
/// chain's identity and scope - so one chain never conflicts with
/// itself through its view - applies the store's strict logical-path
/// rules, and reports error paths as the caller supplied them.
///
/// # Errors
/// Returns an error when the handle the chain's access came from
/// declares no store.
pub fn store_view(access: &Access) -> Result<Access, VfsError> {
    access.store_view()
}

/// Probes the declared store before a run starts: derives the store
/// view from `access` and stats the store root through it, which makes
/// the view's router acquire the store backend. A root the backend
/// reports [`VfsError::NotFound`] passes, so a host directory created
/// lazily still runs.
///
/// # Errors
/// Returns an error when the handle declares no store, or when the
/// store backend refuses the session or fails the stat.
pub fn probe_store(access: &Access) -> Result<(), VfsError> {
    access.store_view()?.probe_root()
}

/// Returns the capability for a new concurrent thread of execution.
///
/// The child gets a fresh [`ExecId`] in `parent`'s scope,
/// and the spawn is the fork: the child shares a snapshot of the parent's
/// vector clock plus its own entry, while the parent's entry advances, so
/// the parent's later accesses are not ordered before the child's.
/// Everything the parent did before the spawn happens before the child's
/// first step. `origin` labels the child's operation events, as in
/// [`VfsRef::acquire`](crate::VfsRef::acquire).
///
/// # Errors
/// Returns an error when the backend refuses to acquire the child's
/// identity. A failed spawn leaves `parent`'s clock untouched.
pub fn access_spawn(parent: &Access, origin: Origin) -> Result<Access, VfsError> {
    parent.spawn(origin)
}

/// Merges `child`'s final clock into `owner`'s: the join, the matching
/// edge of [`access_spawn`]'s fork. Everything `child` did - its writes
/// included - happens before `owner`'s next step, so the owner's reads of
/// the child's files no longer conflict. A child whose identity is still
/// live (its last access not yet dropped) contributes its current clock;
/// a child whose identity ended contributes its final clock, which the
/// scope keeps for exactly this. Joining a timer's identity - one the
/// engine never spawned - is a no-op because no such identity exists.
pub fn access_join(owner: &Access, child: ExecId) {
    owner.join(child);
}

/// The identity `access` was acquired or spawned under: how the engine
/// records a task's [`ExecId`] at spawn, to join it on delivery.
#[must_use]
pub const fn access_id(access: &Access) -> ExecId {
    access.exec_id()
}

#[cfg(test)]
mod tests {
    use super::{access_spawn, end_scope, probe_store, scope_handle, store_view};
    use crate::grep::GrepQuery;
    use crate::path::canonicalize_absolute;
    use crate::{
        Access, AcquireContext, ExecId, MemoryBackend, Origin, PathReason, Vfs, VfsAccess,
        VfsError, VfsPathBuf, VfsRef,
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
    fn a_two_path_backend_error_keeps_a_destination_that_repeats_the_store_root()
    -> Result<(), VfsError> {
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
        // ...and the host directory mounted beneath the store never saw it.
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
    fn the_store_views_grep_runs_the_strict_rules_on_the_root() -> Result<(), VfsError> {
        let vfs = stock();
        let (_access, view) = chain(&vfs);
        let query = |root: VfsPathBuf| GrepQuery {
            pattern: "hit".to_owned(),
            root,
            is_regex: false,
            case_insensitive: false,
            glob_filter: None,
            max_results: None,
        };
        // An absolute root would canonicalize namespace-absolute and
        // register the grep's claims outside the store mount, so the
        // strict rules refuse every absolute form - even one that
        // names the store mount itself - with the root as supplied.
        for root in [
            canonicalize_absolute("/etc")?,
            canonicalize_absolute("/my/store/notes")?,
        ] {
            let root = root.to_buf();
            let reported = root.as_str().to_owned();
            match view.grep(&query(root)) {
                Err(VfsError::InvalidPath {
                    path,
                    reason: PathReason::Absolute,
                }) => assert_eq!(path, reported, "reports the root as supplied"),
                other => {
                    panic!("expected an absolute-root refusal for {reported:?}, got {other:?}")
                }
            }
        }
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
}
