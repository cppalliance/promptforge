//! The parity suite: the store contract ported onto the VFS facade,
//! unmodified in intent. Two tests change shape with the mechanism under
//! test: write-race detection is now the claims model's (a second live
//! identity conflicts; the WriteScope registry is gone), and poison
//! handling is now poison-safe recovery (a panic mid-operation cannot
//! wedge the store) rather than a surfaced backend error.

use promptforge_vfs::STORE_MOUNT;
use shared_vfs::{
    Access, Entry, ExecId, MemoryBackend, Stat, Vfs, VfsAccess, VfsError, VfsPath, VfsRef,
};

use super::path::MAX_STORE_PATH_BYTES;
use super::{MAX_GLOB_PATTERN_BYTES, PathReason, Store, StoreError, StoreErrorKind, StoreExt};

/// A stock handle with one acquired identity: the fixture every
/// single-identity test starts from.
fn stock() -> (VfsRef, Access) {
    let vfs = promptforge_vfs::empty();
    let access = vfs.acquire();
    (vfs, access)
}

#[test]
fn write_then_read_numbered_numbers_lines() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "first\nsecond\nthird").expect("write");
    assert_eq!(
        store
            .read_range_numbered("a.txt", 1, None)
            .expect("numbered"),
        "1| first\n2| second\n3| third"
    );
}

#[test]
fn read_numbered_pads_numbers_to_width() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    numbered_fixture(&store, "a.txt", 10);
    let numbered = store
        .read_range_numbered("a.txt", 1, None)
        .expect("numbered");
    assert!(numbered.starts_with(" 1| line1\n"));
    assert!(numbered.contains("\n10| line10"));
}

#[test]
fn read_returns_contents_verbatim() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "first\nsecond\n").expect("write");
    assert_eq!(store.read("a.txt").expect("read"), "first\nsecond\n");
    assert_eq!(
        store
            .read_range_numbered("a.txt", 1, None)
            .expect("numbered"),
        "1| first\n2| second"
    );
}

#[test]
fn read_missing_file_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    let err = store.read("absent.txt").expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn a_second_identitys_write_to_a_claimed_path_races() {
    // Write-race detection is now the claims model's: a write booms when
    // another live identity holds a claim on the path. One identity
    // rewriting its own path stays legal, and releasing the first
    // identity frees the path.
    let (vfs, first) = stock();
    let store = vfs.store(&first);
    store.write("a.txt", "one").expect("first write");
    store
        .write("a.txt", "uno")
        .expect("one identity may rewrite its own path");
    let second = vfs.acquire();
    let contender = vfs.store(&second);
    let err = contender
        .write("a.txt", "two")
        .expect_err("a second live identity must race");
    assert_eq!(err.kind(), StoreErrorKind::WriteRace);
    assert_eq!(err.path(), Some("a.txt"));
    assert!(
        err.to_string().contains("write-write race"),
        "error was: {err}"
    );
    // The claims model covers appends, which WriteScope never did.
    let err = contender
        .append("a.txt", "+")
        .expect_err("a second live identity's append must race");
    assert_eq!(err.kind(), StoreErrorKind::WriteRace);
    // The raced writes never reached the backend.
    assert_eq!(store.read("a.txt").expect("read"), "uno");
    // Releasing the first identity retires its claims; the path is free.
    drop(first);
    contender
        .write("a.txt", "new")
        .expect("a released claim no longer conflicts");
    assert_eq!(contender.read("a.txt").expect("read"), "new");
}

#[test]
fn a_glob_over_a_claimed_path_races_like_a_read() {
    // Error-mapping parity: a read booms when another live identity holds
    // a writer claim, and a glob (or its per-match stat) that touches the
    // same claimed path must surface the same WriteRace vocabulary, not an
    // opaque Backend.
    let (vfs, first) = stock();
    let store = vfs.store(&first);
    store.write("a.txt", "one").expect("first write");
    let second = vfs.acquire();
    let contender = vfs.store(&second);
    let err = contender
        .glob("*.txt")
        .expect_err("a glob touching a claimed path must race");
    assert_eq!(err.kind(), StoreErrorKind::WriteRace);
    // Releasing the first identity retires its claims; the glob succeeds.
    drop(first);
    assert_eq!(contender.glob("*.txt").expect("glob"), vec!["a.txt"]);
}

#[test]
fn two_facades_bound_to_one_identity_never_conflict() {
    // Borrow semantics: blocking call chains share the parent's access,
    // so two facades over one identity touch one path without a false
    // conflict.
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    let also = store.clone();
    store.write("a.txt", "one").expect("write");
    also.append("a.txt", "two").expect("append");
    assert_eq!(store.read("a.txt").expect("read"), "onetwo");
}

#[test]
fn identities_share_backing_state_once_claims_are_released() {
    let (vfs, first) = stock();
    let store = vfs.store(&first);
    store
        .write("shared.txt", "written by the first")
        .expect("write");
    drop(first);
    let second = vfs.acquire();
    let reader = vfs.store(&second);
    assert_eq!(
        reader.read("shared.txt").expect("read"),
        "written by the first"
    );
}

#[test]
fn read_range_with_start_only_reads_to_end() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store.read_range("a.txt", 2, None).expect("read_range"),
        "two\nthree"
    );
}

#[test]
fn read_range_with_start_and_end_slices_inclusively() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store.read_range("a.txt", 2, Some(2)).expect("read_range"),
        "two"
    );
    assert_eq!(
        store.read_range("a.txt", 1, Some(2)).expect("read_range"),
        "one\ntwo"
    );
}

#[test]
fn read_range_clamps_end_to_the_last_line() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store.read_range("a.txt", 2, Some(99)).expect("read_range"),
        "two\nthree"
    );
}

#[test]
fn read_range_beyond_eof_is_empty() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(store.read_range("a.txt", 4, None).expect("read_range"), "");
    // The end bound is never evaluated when the range starts beyond EOF.
    assert_eq!(
        store.read_range("a.txt", 4, Some(1)).expect("read_range"),
        ""
    );
}

#[test]
fn read_range_empty_file_is_empty_string() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("e.txt", "").expect("write");
    assert_eq!(store.read_range("e.txt", 1, None).expect("read_range"), "");
}

#[test]
fn read_range_start_below_one_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\n").expect("write");
    for style in [RangeStyle::Plain, RangeStyle::Numbered] {
        let err = style
            .read(&store, "a.txt", 0, None)
            .expect_err("start of 0");
        assert_eq!(err.kind(), StoreErrorKind::InvalidRange, "{style:?}");
        assert!(matches!(err, StoreError::InvalidRange { .. }));
        assert_eq!(err.path(), Some("a.txt"));
    }
}

#[test]
fn read_range_end_before_start_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    for style in [RangeStyle::Plain, RangeStyle::Numbered] {
        let err = style
            .read(&store, "a.txt", 3, Some(2))
            .expect_err("end before start");
        assert_eq!(err.kind(), StoreErrorKind::InvalidRange, "{style:?}");
        assert!(matches!(err, StoreError::InvalidRange { .. }));
    }
}

#[test]
fn read_range_missing_file_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    for style in [RangeStyle::Plain, RangeStyle::Numbered] {
        let err = style
            .read(&store, "absent.txt", 1, None)
            .expect_err("should fail");
        assert!(matches!(err, StoreError::NotFound { .. }), "{style:?}");
    }
}

#[derive(Clone, Copy, Debug)]
enum RangeStyle {
    Plain,
    Numbered,
}

impl RangeStyle {
    fn read(
        self,
        store: &Store,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, StoreError> {
        match self {
            Self::Plain => store.read_range(path, start, end),
            Self::Numbered => store.read_range_numbered(path, start, end),
        }
    }
}

/// Writes `line1` through `line<line_count>` into `path`.
fn numbered_fixture(store: &Store, path: &str, line_count: usize) {
    let mut body = String::new();
    for n in 1..=line_count {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write(path, &body).expect("write");
}

#[test]
fn read_range_numbered_without_bounds_numbers_from_one() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    numbered_fixture(&store, "a.txt", 12);
    assert_eq!(
        store
            .read_range_numbered("a.txt", 1, None)
            .expect("numbered"),
        " 1| line1\n 2| line2\n 3| line3\n 4| line4\n 5| line5\n 6| line6\n 7| line7\n 8| line8\n 9| line9\n10| line10\n11| line11\n12| line12"
    );
}

#[test]
fn read_range_numbered_empty_file_is_empty_string() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("e.txt", "").expect("write");
    assert_eq!(
        store
            .read_range_numbered("e.txt", 1, None)
            .expect("numbered"),
        ""
    );
}

#[test]
fn read_range_numbered_numbers_a_slice_absolutely() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    numbered_fixture(&store, "a.txt", 85);
    assert_eq!(
        store
            .read_range_numbered("a.txt", 84, Some(85))
            .expect("numbered"),
        "84| line84\n85| line85"
    );
}

#[test]
fn read_range_numbered_pads_across_the_hundred_boundary() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    numbered_fixture(&store, "a.txt", 100);
    assert_eq!(
        store
            .read_range_numbered("a.txt", 99, Some(100))
            .expect("numbered"),
        " 99| line99\n100| line100"
    );
}

#[test]
fn read_range_numbered_clamps_end_to_the_last_line() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    numbered_fixture(&store, "a.txt", 100);
    assert_eq!(
        store
            .read_range_numbered("a.txt", 99, Some(999))
            .expect("numbered"),
        " 99| line99\n100| line100"
    );
    assert_eq!(
        store
            .read_range_numbered("a.txt", 2, None)
            .expect("numbered"),
        store
            .read_range_numbered("a.txt", 2, Some(100))
            .expect("numbered"),
        "an omitted end must mean the last line"
    );
}

#[test]
fn read_range_numbered_beyond_eof_is_empty() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store
            .read_range_numbered("a.txt", 4, None)
            .expect("numbered"),
        ""
    );
}

#[test]
fn write_overwrites() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "old").expect("write");
    store.write("a.txt", "new").expect("overwrite");
    assert_eq!(store.read("a.txt").expect("read"), "new");
}

#[test]
fn append_creates_then_extends() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.append("log.txt", "one\n").expect("create via append");
    store.append("log.txt", "two").expect("extend");
    assert_eq!(store.read("log.txt").expect("read"), "one\ntwo");
}

#[test]
fn str_replace_replaces_unique() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "the quick brown fox").expect("write");
    store
        .str_replace("a.txt", "quick", "slow")
        .expect("replace");
    assert_eq!(store.read("a.txt").expect("read"), "the slow brown fox");
}

#[test]
fn str_replace_missing_anchor_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "hello world").expect("write");
    let err = store
        .str_replace("a.txt", "absent", "x")
        .expect_err("should fail");
    assert!(matches!(err, StoreError::AnchorNotFound { .. }));
}

#[test]
fn str_replace_ambiguous_anchor_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "na na na").expect("write");
    let err = store
        .str_replace("a.txt", "na", "la")
        .expect_err("should fail");
    match err {
        StoreError::AnchorAmbiguous { count, .. } => assert_eq!(count, 3),
        other => panic!("expected ambiguous, got {other:?}"),
    }
}

#[test]
fn str_replace_on_missing_file_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    let err = store
        .str_replace("nope.txt", "a", "b")
        .expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn delete_then_read_errors() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "gone soon").expect("write");
    store.delete("a.txt").expect("delete");
    let err = store.read("a.txt").expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn delete_missing_is_silent() {
    // Delete is idempotent: deleting an absent path succeeds.
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.delete("absent.txt").expect("delete is idempotent");
}

#[test]
fn glob_matches_sorted() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    for path in ["src/b.rs", "src/a.rs", "src/deep/c.rs", "notes.md"] {
        store.write(path, "").expect("write");
    }
    assert_eq!(
        store.glob("src/*.rs").expect("glob"),
        vec!["src/a.rs", "src/b.rs"]
    );
    assert_eq!(
        store.glob("src/**/*.rs").expect("glob"),
        vec!["src/a.rs", "src/b.rs", "src/deep/c.rs"],
    );
    assert_eq!(store.glob("*.md").expect("glob"), vec!["notes.md"]);
    // The store vocabulary lists files only: the materialized ancestor
    // directories the VFS glob also matches are filtered out.
    assert_eq!(store.glob("**").expect("glob").len(), 4);
}

#[test]
fn glob_star_stops_at_slash() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a/b.txt", "").expect("write");
    assert!(store.glob("*.txt").expect("glob").is_empty());
    assert_eq!(store.glob("a/*.txt").expect("glob"), vec!["a/b.txt"]);
}

fn assert_invalid_paths(store: &Store, cases: &[(&str, PathReason)]) {
    for (path, reason) in cases {
        let err = store.read(path).expect_err("path must be rejected");
        assert_eq!(err.kind(), StoreErrorKind::InvalidPath, "{path}");
        match err {
            StoreError::InvalidPath { reason: got, .. } => assert_eq!(&got, reason, "{path}"),
            other => panic!("expected InvalidPath for {path:?}, got {other:?}"),
        }
    }
}

#[test]
fn invalid_paths_are_rejected_before_dispatch() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    assert_invalid_paths(
        &store,
        &[
            ("", PathReason::Empty),
            ("/abs.txt", PathReason::Absolute),
            ("../escape.txt", PathReason::Traversal),
            ("a/./b.txt", PathReason::Traversal),
            ("a//b.txt", PathReason::EmptySegment),
            ("a\u{0}b.txt", PathReason::Control),
        ],
    );
}

#[test]
fn exists_reports_confirmed_absence_and_presence() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    assert!(!store.exists("a.txt").expect("absence is not an error"));
    store.write("a.txt", "hi").expect("write");
    assert!(store.exists("a.txt").expect("presence is not an error"));
}

#[test]
fn glob_rejects_empty_and_oversized_and_control_patterns() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    assert_eq!(
        store.glob("").expect_err("empty").kind(),
        StoreErrorKind::InvalidPattern
    );
    let huge = "a".repeat(MAX_GLOB_PATTERN_BYTES + 1);
    assert_eq!(
        store.glob(&huge).expect_err("oversize").kind(),
        StoreErrorKind::InvalidPattern
    );
    assert_eq!(
        store.glob("a\u{0}b").expect_err("control").kind(),
        StoreErrorKind::InvalidPattern
    );
}

#[test]
fn empty_anchor_is_refused() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a.txt", "body").expect("write");
    let err = store
        .str_replace("a.txt", "", "x")
        .expect_err("empty anchor");
    // STORE-007: an empty anchor is a dedicated invalid-anchor condition,
    // distinct from an anchor that was searched for and not found.
    assert_eq!(err.kind(), StoreErrorKind::InvalidAnchor);
    assert!(matches!(err, StoreError::InvalidAnchor { .. }));
}

#[test]
fn str_replace_reports_empty_ascii_and_multibyte_contents() {
    // STORE-007 coverage: empty, ASCII, and multibyte file contents.
    let (vfs, access) = stock();
    let store = vfs.store(&access);

    store.write("empty.txt", "").expect("write empty");
    let empty_err = store
        .str_replace("empty.txt", "x", "y")
        .expect_err("anchor absent in empty file");
    assert!(matches!(empty_err, StoreError::AnchorNotFound { .. }));

    store
        .write("ascii.txt", "one two three")
        .expect("write ascii");
    store
        .str_replace("ascii.txt", "two", "TWO")
        .expect("ascii anchor replaced");
    assert_eq!(store.read("ascii.txt").expect("read"), "one TWO three");

    store.write("multi.txt", "café résumé café").expect("write");
    let ambiguous = store
        .str_replace("multi.txt", "café", "COFFEE")
        .expect_err("multibyte anchor occurs twice");
    assert!(matches!(
        ambiguous,
        StoreError::AnchorAmbiguous { count: 2, .. }
    ));
    store
        .str_replace("multi.txt", "résumé", "CV")
        .expect("unique multibyte anchor replaced");
    assert_eq!(store.read("multi.txt").expect("read"), "café CV café");
}

#[test]
fn platform_unsafe_paths_are_rejected_before_dispatch() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    assert_invalid_paths(
        &store,
        &[
            ("a\\b.txt", PathReason::Backslash),
            ("CON", PathReason::ReservedName),
            ("dir/nul.txt", PathReason::ReservedName),
            ("com1", PathReason::ReservedName),
            ("LPT9.log", PathReason::ReservedName),
            ("trailing.", PathReason::UnsafeSuffix),
            ("trailing ", PathReason::UnsafeSuffix),
        ],
    );
    // A path at the exact byte limit is accepted; one byte over is rejected.
    let maximum = "a".repeat(MAX_STORE_PATH_BYTES);
    store
        .write(&maximum, "at-limit")
        .expect("a 1024-byte path must be accepted");
    assert_eq!(
        store.read(&maximum).expect("read at-limit path"),
        "at-limit"
    );
    let too_long = "a".repeat(MAX_STORE_PATH_BYTES + 1);
    let error = store
        .read(&too_long)
        .expect_err("a 1025-byte path must be rejected");
    assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
    assert!(matches!(
        error,
        StoreError::InvalidPath {
            reason: PathReason::TooLong,
            ..
        }
    ));
    // Names that merely contain a device substring are allowed.
    store
        .write("console.txt", "ok")
        .expect("console is not CON");
    store.write("com10.txt", "ok").expect("com10 is not com1");
}

#[test]
fn glob_grammar_rejects_unsupported_forms() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    for bad in ["a**b", "***", "a/***/b", "a\\*.txt"] {
        assert_eq!(
            store.glob(bad).expect_err(bad).kind(),
            StoreErrorKind::InvalidPattern,
            "{bad}"
        );
    }
    // Well-formed `**` placements are accepted.
    for good in ["**", "**/x", "a/**", "a/**/b", "src/*.rs"] {
        store.glob(good).expect(good);
    }
}

#[test]
fn glob_matcher_is_bounded_against_adversarial_patterns() {
    // STORE-005: a pattern packed with single-segment stars against a long
    // non-matching name completes promptly (a recursive/backtracking
    // matcher would blow up here). The iterative matcher is O(tokens*len).
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    let name = "a".repeat(200);
    store.write(&name, "").expect("write");
    // A grammar-valid pattern of many single `*` separated by literals: the
    // classic exponential-backtracking trap for a naive recursive matcher.
    // With no trailing 'b' in the text it cannot match, and the iterative
    // matcher must still return promptly.
    let pattern = format!("{}*b", "*a".repeat(40));
    assert!(store.glob(&pattern).expect("bounded glob").is_empty());

    // `**` spanning slashes and `**/` matching zero segments both hold.
    store.write("x/y/z.rs", "").expect("write nested");
    assert_eq!(store.glob("x/**/z.rs").expect("glob"), vec!["x/y/z.rs"]);
    store.write("z2.rs", "").expect("write top");
    assert!(
        store
            .glob("**/z2.rs")
            .expect("glob")
            .contains(&"z2.rs".to_owned())
    );
}

#[test]
fn glob_double_star_slash_matches_zero_segments() {
    let (vfs, access) = stock();
    let store = vfs.store(&access);
    store.write("a/b.rs", "").expect("write");
    // `a/**/b.rs` matches `a/b.rs` (zero intermediate segments).
    assert_eq!(store.glob("a/**/b.rs").expect("glob"), vec!["a/b.rs"]);
}

#[test]
fn backend_ctor_classifies_and_hides_source() {
    let err = StoreError::backend(std::io::Error::other("disk gone"));
    assert_eq!(err.kind(), StoreErrorKind::Backend);
    assert!(err.path().is_none());
    assert!(std::error::Error::source(&err).is_some());
    assert!(
        !err.to_string().contains("disk gone"),
        "Display must not expose the backend source: {err}"
    );
}

#[test]
fn the_facade_is_send_and_sync() {
    // The facade is shared across spawned tasks that outlive the caller,
    // so the assertion carries the promised bounds.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store<'static>>();
}

/// A backend whose writes panic mid-call, poisoning every lock on the
/// operation path. All other operations delegate to a memory backend.
struct PanicBackend(MemoryBackend);

impl Vfs for PanicBackend {
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        Ok(Box::new(PanicAccess(self.0.acquire(id)?)))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.0.release(id)
    }
}

struct PanicAccess(Box<dyn VfsAccess>);

impl VfsAccess for PanicAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.0.read(path)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        let _ = (path, contents);
        panic!("poison the operation path");
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.append(path, contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.remove(path, recursive)
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.0.exists(path)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.0.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.0.list(path)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.0.stat(path)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.mkdir(path, recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.rename(from, to)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.copy(from, to)
    }
}

#[test]
fn a_panicking_operation_does_not_wedge_the_store() {
    // Poison handling is now poison-safe recovery: a panic mid-operation
    // poisons the locks on the operation path, the poison-safe locking
    // recovers, and the very next operation works.
    let vfs = VfsRef::builder()
        .mount(STORE_MOUNT, PanicBackend(MemoryBackend::new()))
        .build();
    let access = vfs.acquire();
    let store = vfs.store(&access);
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.write("a.txt", "x")));
    assert!(outcome.is_err(), "the panic must propagate");
    store.append("b.txt", "y").expect("append after the panic");
    assert_eq!(store.read("b.txt").expect("read"), "y");
}
