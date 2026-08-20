use super::*;

/// A backend whose `glob` ignores the pattern and always returns every path.
///
/// If `StoreRef::glob` delegated matching to the backend it would return all
/// paths unfiltered; the filtered results below prove `StoreRef` applies the
/// caller's pattern to the backend snapshot itself.
struct GlobSpyStore {
    paths: Vec<String>,
}

impl Store for GlobSpyStore {
    fn write(&mut self, _path: &str, _contents: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn append(&mut self, _path: &str, _contents: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn read(&self, _path: &str) -> Result<String, StoreError> {
        Ok(String::new())
    }
    fn str_replace(&mut self, _path: &str, _old: &str, _new: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn delete(&mut self, _path: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn glob(&self, _pattern: &str) -> Result<Vec<String>, StoreError> {
        Ok(self.paths.clone())
    }
    fn exists(&self, _path: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
}

#[test]
fn glob_filters_backend_snapshot_with_caller_pattern() {
    let spy = GlobSpyStore {
        paths: vec![
            "src/a.rs".to_owned(),
            "src/b.md".to_owned(),
            "src/deep/c.rs".to_owned(),
        ],
    };
    let store = StoreRef::new(Box::new(spy));
    // The caller's real pattern is applied by `StoreRef` to the backend result.
    let matched = store.glob("src/*.rs").expect("glob");
    assert_eq!(matched, vec!["src/a.rs".to_owned()]);
    let matched = store.glob("src/**/*.rs").expect("glob");
    assert_eq!(
        matched,
        vec!["src/a.rs".to_owned(), "src/deep/c.rs".to_owned()]
    );
}

#[test]
fn write_then_read_numbered_numbers_lines() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    let mut body = String::new();
    for n in 1..=10 {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write("a.txt", &body).expect("write");
    let numbered = store
        .read_range_numbered("a.txt", 1, None)
        .expect("numbered");
    assert!(numbered.starts_with(" 1| line1\n"));
    assert!(numbered.contains("\n10| line10"));
}

#[test]
fn read_returns_contents_verbatim() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    let err = store.read("absent.txt").expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn scoped_writes_reject_a_second_arm_of_one_fanout() {
    // Note 74: the write registry records each scoped write's (fanout token,
    // arm index). A second arm of the same fanout writing the same path is a
    // hard write-write race; the same arm rewriting, a later fanout's write,
    // and untracked writes all stay legal.
    let store = StoreRef::memory();
    let token = store.next_write_token();
    let arm_one = WriteScope::new(token, 1);
    let arm_two = WriteScope::new(token, 2);
    store
        .write_scoped("a.txt", "one", arm_one)
        .expect("first write");
    store
        .write_scoped("a.txt", "uno", arm_one)
        .expect("the same arm may rewrite its own path");
    let err = store
        .write_scoped("a.txt", "two", arm_two)
        .expect_err("a second arm of the same fanout must race");
    assert_eq!(err.kind(), StoreErrorKind::WriteRace);
    assert_eq!(err.path(), Some("a.txt"));
    assert!(
        err.to_string().contains("another arm of the same fanout"),
        "error was: {err}"
    );
    // The raced write never reached the backend.
    assert_eq!(store.read("a.txt").expect("read"), "uno");
    // A later fanout overwrites the record, so sequential fanouts stay legal.
    let later = WriteScope::new(store.next_write_token(), 1);
    store
        .write_scoped("a.txt", "new", later)
        .expect("a later fanout may write the path");
    assert_eq!(store.read("a.txt").expect("read"), "new");
    // Untracked writes neither record nor race.
    store
        .write("a.txt", "walk")
        .expect("walk-section writes are untracked");
    store.append("a.txt", "+").expect("append is untracked");
    assert_eq!(store.read("a.txt").expect("read"), "walk+");
}

#[test]
fn read_range_with_start_only_reads_to_end() {
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store.read_range("a.txt", 2, None).expect("read_range"),
        "two\nthree"
    );
}

#[test]
fn read_range_with_start_and_end_slices_inclusively() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store.read_range("a.txt", 2, Some(99)).expect("read_range"),
        "two\nthree"
    );
}

#[test]
fn read_range_beyond_eof_is_empty() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    store.write("e.txt", "").expect("write");
    assert_eq!(store.read_range("e.txt", 1, None).expect("read_range"), "");
}

#[test]
fn read_range_start_below_one_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\n").expect("write");
    let err = store.read_range("a.txt", 0, None).expect_err("start of 0");
    assert_eq!(err.kind(), StoreErrorKind::InvalidRange);
    assert!(matches!(err, StoreError::InvalidRange { .. }));
    assert_eq!(err.path(), Some("a.txt"));
}

#[test]
fn read_range_end_before_start_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    let err = store
        .read_range("a.txt", 3, Some(2))
        .expect_err("end before start");
    assert_eq!(err.kind(), StoreErrorKind::InvalidRange);
    assert!(matches!(err, StoreError::InvalidRange { .. }));
}

#[test]
fn read_range_missing_file_errors() {
    let store = StoreRef::memory();
    let err = store
        .read_range("absent.txt", 1, None)
        .expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

/// Writes `line1` through `line<line_count>` into `path`.
fn numbered_fixture(store: &StoreRef, path: &str, line_count: usize) {
    let mut body = String::new();
    for n in 1..=line_count {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write(path, &body).expect("write");
}

#[test]
fn read_range_numbered_without_bounds_numbers_from_one() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    assert_eq!(
        store
            .read_range_numbered("a.txt", 4, None)
            .expect("numbered"),
        ""
    );
}

#[test]
fn read_range_numbered_start_below_one_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\n").expect("write");
    let err = store
        .read_range_numbered("a.txt", 0, None)
        .expect_err("start of 0");
    assert_eq!(err.kind(), StoreErrorKind::InvalidRange);
    assert!(matches!(err, StoreError::InvalidRange { .. }));
    assert_eq!(err.path(), Some("a.txt"));
}

#[test]
fn read_range_numbered_end_before_start_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "one\ntwo\nthree\n").expect("write");
    let err = store
        .read_range_numbered("a.txt", 3, Some(2))
        .expect_err("end before start");
    assert_eq!(err.kind(), StoreErrorKind::InvalidRange);
    assert!(matches!(err, StoreError::InvalidRange { .. }));
}

#[test]
fn read_range_numbered_missing_file_errors() {
    let store = StoreRef::memory();
    let err = store
        .read_range_numbered("absent.txt", 1, None)
        .expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn write_overwrites() {
    let store = StoreRef::memory();
    store.write("a.txt", "old").expect("write");
    store.write("a.txt", "new").expect("overwrite");
    assert_eq!(store.read("a.txt").expect("read"), "new");
}

#[test]
fn append_creates_then_extends() {
    let store = StoreRef::memory();
    store.append("log.txt", "one\n").expect("create via append");
    store.append("log.txt", "two").expect("extend");
    assert_eq!(store.read("log.txt").expect("read"), "one\ntwo");
}

#[test]
fn str_replace_replaces_unique() {
    let store = StoreRef::memory();
    store.write("a.txt", "the quick brown fox").expect("write");
    store
        .str_replace("a.txt", "quick", "slow")
        .expect("replace");
    assert_eq!(store.read("a.txt").expect("read"), "the slow brown fox");
}

#[test]
fn str_replace_missing_anchor_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "hello world").expect("write");
    let err = store
        .str_replace("a.txt", "absent", "x")
        .expect_err("should fail");
    assert!(matches!(err, StoreError::AnchorNotFound { .. }));
}

#[test]
fn str_replace_ambiguous_anchor_errors() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
    let err = store
        .str_replace("nope.txt", "a", "b")
        .expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn delete_then_read_errors() {
    let store = StoreRef::memory();
    store.write("a.txt", "gone soon").expect("write");
    store.delete("a.txt").expect("delete");
    let err = store.read("a.txt").expect_err("should fail");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn delete_missing_is_silent() {
    // Note 55: delete is idempotent, so deleting an absent path succeeds.
    let store = StoreRef::memory();
    store.delete("absent.txt").expect("delete is idempotent");
}

#[test]
fn glob_matches_sorted() {
    let store = StoreRef::memory();
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
    assert_eq!(store.glob("**").expect("glob").len(), 4);
}

#[test]
fn glob_star_stops_at_slash() {
    let store = StoreRef::memory();
    store.write("a/b.txt", "").expect("write");
    assert!(store.glob("*.txt").expect("glob").is_empty());
    assert_eq!(store.glob("a/*.txt").expect("glob"), vec!["a/b.txt"]);
}

#[test]
fn invalid_paths_are_rejected_before_dispatch() {
    let store = StoreRef::memory();
    for (path, reason) in [
        ("", PathReason::Empty),
        ("/abs.txt", PathReason::Absolute),
        ("../escape.txt", PathReason::Traversal),
        ("a/./b.txt", PathReason::Traversal),
        ("a//b.txt", PathReason::EmptySegment),
        ("a\u{0}b.txt", PathReason::Control),
    ] {
        let err = store.read(path).expect_err("path must be rejected");
        assert_eq!(err.kind(), StoreErrorKind::InvalidPath, "{path}");
        match err {
            StoreError::InvalidPath { reason: got, .. } => assert_eq!(got, reason, "{path}"),
            other => panic!("expected InvalidPath for {path:?}, got {other:?}"),
        }
    }
}

#[test]
fn exists_reports_confirmed_absence_and_presence() {
    let store = StoreRef::memory();
    assert!(!store.exists("a.txt").expect("absence is not an error"));
    store.write("a.txt", "hi").expect("write");
    assert!(store.exists("a.txt").expect("presence is not an error"));
}

#[test]
fn glob_rejects_empty_and_oversized_and_control_patterns() {
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();

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
    let store = StoreRef::memory();
    for (path, reason) in [
        ("a\\b.txt", PathReason::Backslash),
        ("CON", PathReason::ReservedName),
        ("dir/nul.txt", PathReason::ReservedName),
        ("com1", PathReason::ReservedName),
        ("LPT9.log", PathReason::ReservedName),
        ("trailing.", PathReason::UnsafeSuffix),
        ("trailing ", PathReason::UnsafeSuffix),
    ] {
        let err = store.read(path).expect_err("path must be rejected");
        assert_eq!(err.kind(), StoreErrorKind::InvalidPath, "{path}");
        match err {
            StoreError::InvalidPath { reason: got, .. } => assert_eq!(got, reason, "{path}"),
            other => panic!("expected InvalidPath for {path:?}, got {other:?}"),
        }
    }
    // A path at the exact byte limit is accepted; one byte over is rejected.
    let maximum = "a".repeat(path::MAX_STORE_PATH_BYTES);
    store
        .write(&maximum, "at-limit")
        .expect("a 1024-byte path must be accepted");
    assert_eq!(
        store.read(&maximum).expect("read at-limit path"),
        "at-limit"
    );
    let too_long = "a".repeat(path::MAX_STORE_PATH_BYTES + 1);
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
    let store = StoreRef::memory();
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
    // non-matching name completes promptly (the old recursive/backtracking
    // matcher would blow up here). The iterative matcher is O(tokens*len).
    let store = StoreRef::memory();
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
    let store = StoreRef::memory();
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
fn store_ref_is_send_sync_and_static() {
    // STORE-009: the handle must also be `'static` (it is shared across
    // spawned tasks that outlive the caller), so the assertion carries the
    // promised `'static` bound, not just `Send + Sync`.
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<StoreRef>();
}

#[test]
fn a_poisoned_backend_lock_surfaces_as_a_backend_error() {
    let store = StoreRef::memory();
    let clone = store.clone();
    let _ = std::thread::spawn(move || {
        let _guard = clone.inner.lock().expect("lock");
        panic!("poison the store lock");
    })
    .join();
    // STORE-004: after a holder panicked mid-hold, operations report a
    // backend failure rather than trusting the possibly half-mutated state.
    let err = store.read("a.txt").expect_err("a poisoned lock must error");
    assert_eq!(err.kind(), StoreErrorKind::Backend);
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn with_files_populates_store() {
    let store = StoreRef::with_files([
        ("a.txt".to_owned(), "alpha".to_owned()),
        ("b.txt".to_owned(), "beta".to_owned()),
    ])
    .expect("valid paths");
    assert_eq!(store.read("a.txt").unwrap(), "alpha");
    assert_eq!(store.read("b.txt").unwrap(), "beta");
}

#[test]
fn with_files_rejects_invalid_path() {
    let result = StoreRef::with_files([("../escape.txt".to_owned(), "bad".to_owned())]);
    assert!(result.is_err());
}

#[test]
fn with_files_empty_is_empty_store() {
    let store =
        StoreRef::with_files(std::iter::empty::<(String, String)>()).expect("empty is valid");
    assert!(!store.exists("anything.txt").unwrap());
}

#[test]
fn clones_share_backing_state() {
    let store = StoreRef::memory();
    let clone = store.clone();
    store
        .write("shared.txt", "written by original")
        .expect("write");
    assert_eq!(
        clone.read("shared.txt").expect("read"),
        "written by original"
    );
    clone
        .write("second.txt", "written by clone")
        .expect("write");
    assert_eq!(store.read("second.txt").expect("read"), "written by clone");
}
