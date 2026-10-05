use super::*;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// A crate root under the system temporary directory that removes itself
/// on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "build-ceiling-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).expect("the fixture's src directory creates");
        Fixture(root)
    }

    /// Writes `bytes` to the `/`-separated `relative` under the root and
    /// returns its path.
    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = relative
            .split('/')
            .fold(self.0.clone(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().expect("a fixture file has a parent"))
            .expect("the fixture directory creates");
        fs::write(&path, bytes).expect("the fixture file writes");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `count` lines of `// line`, each ending in a newline.
fn lines(count: usize) -> String {
    "// line\n".repeat(count)
}

/// The oversized files a scan reported, as `(path, lines)`.
fn oversized(scan: &Scan) -> Vec<(PathBuf, usize)> {
    scan.findings
        .iter()
        .filter_map(|finding| match finding {
            Finding::Oversized { path, lines } => Some((path.clone(), *lines)),
            Finding::Unreadable { .. } => None,
        })
        .collect()
}

#[test]
fn every_physical_line_counts_blank_comment_crlf_and_unterminated() {
    let fixture = Fixture::new();
    let text = format!(
        "{}{}fn last_line_without_a_newline() {{}}",
        "\r\n".repeat(250),
        "// comment\r\n".repeat(250)
    );
    let path = fixture.write("src/lib.rs", text);
    assert_eq!(oversized(&scan(&fixture.0)), vec![(path, 501)]);
}

#[test]
fn a_file_at_the_limit_passes_and_one_line_over_fails() {
    let fixture = Fixture::new();
    fixture.write("src/at.rs", lines(500));
    let over = fixture.write("src/over.rs", lines(501));
    let scan = scan(&fixture.0);
    assert_eq!(oversized(&scan), vec![(over, 501)]);
    assert_eq!(scan.findings.len(), 1, "only the file over the limit fails");
}

#[test]
fn every_counted_tree_and_the_build_script_are_scanned_and_nothing_else() {
    let fixture = Fixture::new();
    let counted = [
        "src/nested/deep/mod.rs",
        "tests/it/main.rs",
        "benches/bench.rs",
        "examples/demo.rs",
        "build.rs",
    ]
    .map(|relative| fixture.write(relative, lines(501)));
    fixture.write("ui/generated.rs", lines(501));
    fixture.write("src/notes.md", lines(501));
    let mut found: Vec<PathBuf> = oversized(&scan(&fixture.0))
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    found.sort();
    let mut expected = counted.to_vec();
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn only_paths_that_exist_are_watched() {
    let fixture = Fixture::new();
    fixture.write("tests/it/main.rs", lines(1));
    let mut watched = scan(&fixture.0).watched;
    watched.sort();
    assert_eq!(
        watched,
        vec![fixture.0.join("src"), fixture.0.join("tests")],
        "a missing path makes cargo rerun the build script on every build, so only \
         existing directories and build.rs are watched"
    );
    fixture.write("build.rs", lines(1));
    assert!(
        scan(&fixture.0)
            .watched
            .contains(&fixture.0.join("build.rs"))
    );
}

#[test]
fn display_and_debug_both_carry_the_whole_failure_message() {
    let fixture = Fixture::new();
    let path = fixture.write("src/big.rs", lines(501));
    let violations = Violations {
        findings: scan(&fixture.0).findings,
    };
    for text in [format!("{violations}"), format!("{violations:?}")] {
        for part in [
            path.display().to_string().as_str(),
            "has 501 lines",
            "limit of 500",
            "help: split the file only into private child modules of that file",
            "help: declare each child inside the file as `#[path = \"<stem>-<topic>.rs\"] mod <topic>;`",
            "help: open each child module with a `//!` line naming its one concern",
            "help: move an inline `#[cfg(test)]` module to `<stem>-tests.rs` first",
            "help: reach the parent's private items through `super::`",
            "help: never widen anything to `pub(crate)` or `pub` to make a split compile",
        ] {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
        assert!(
            text.ends_with(
                "If no cohesive group splits out without widening visibility, stop and say why."
            ),
            "the message ends with the stop sentence:\n{text}"
        );
    }
}

#[test]
fn an_unreadable_file_is_reported_not_skipped() {
    let fixture = Fixture::new();
    let path = fixture.write("src/bad.rs", [b'f', 0xff, 0xfe, b'\n']);
    let findings = scan(&fixture.0).findings;
    assert!(
        matches!(findings.as_slice(), [Finding::Unreadable { path: reported, .. }] if *reported == path),
        "the invalid UTF-8 file is reported"
    );
    let text = Violations { findings }.to_string();
    assert!(text.contains(&path.display().to_string()), "{text}");
}
