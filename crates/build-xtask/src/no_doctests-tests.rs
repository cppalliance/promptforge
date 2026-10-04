//! The no-doctests check over this workspace, plus fixtures that each
//! compiled block form fails at its opening line, each block rustdoc never
//! compiles passes, and every source in every crate is scanned.

use std::path::{Path, PathBuf};

use super::*;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

fn check(text: &str) -> Vec<String> {
    source_violations(Path::new("lib.rs"), text)
}

/// Asserts `text` has exactly one violation, a doctest at `line`.
fn assert_doctest_at(text: &str, line: usize) {
    let violations = check(text);
    assert_eq!(violations.len(), 1, "{text}\n{violations:?}");
    assert!(
        violations[0].starts_with(&format!("lib.rs:{line}: {RULE}: required ")),
        "{text}\n{violations:?}"
    );
}

fn assert_clean(text: &str) {
    let violations = check(text);
    assert!(violations.is_empty(), "{text}\n{violations:?}");
}

/// `path`, written with `/`, under `root` with the platform's separators,
/// so it compares equal to a path the scan built.
fn at(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// A fake workspace root holding each `(path, text)`.
fn tree(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    for (path, text) in files {
        let file = at(root.path(), path);
        let dir = file.parent().expect("a fixture file has a directory");
        std::fs::create_dir_all(dir).expect("the fixture directory creates");
        std::fs::write(&file, text).expect("the fixture file writes");
    }
    root
}

const MANIFEST: &[u8] = b"[package]\nname = \"tool\"\n";
const DOCTEST: &[u8] = b"/// Runs.\n///\n/// ```\n/// run();\n/// ```\npub fn run() {}\n";

#[test]
fn the_workspace_has_no_doctests() {
    let violations = no_doctests_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "doctest violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn an_untagged_fence_is_rejected_at_its_opening_line() {
    for (text, line) in [
        (
            "/// Adds.\n///\n/// ```\n/// assert_eq!(add(1, 2), 3);\n/// ```\npub fn add(a: u32, b: u32) -> u32 { a + b }\n",
            3,
        ),
        ("//! Crate.\n//!\n//! ```\n//! let x = 1;\n//! ```\n", 3),
        ("#[doc = \"```\\nlet x = 1;\\n```\"]\npub struct Seam;\n", 1),
        (
            "pub struct Wire {\n    /// ```\n    /// let wire = 1;\n    /// ```\n    pub name: String,\n}\n",
            2,
        ),
    ] {
        assert_doctest_at(text, line);
    }
}

#[test]
fn a_fence_with_a_rust_tag_is_rejected_at_its_opening_line() {
    for tag in [
        "rust",
        "no_run",
        "should_panic",
        "compile_fail",
        "ignore",
        "test_harness",
        "standalone_crate",
        "ignore-windows",
        "edition2024",
        "rust,ignore",
        "no_run edition2021",
        "should_panic,text",
    ] {
        assert_doctest_at(
            &format!("/// Runs.\n///\n/// ```{tag}\n/// run();\n/// ```\npub fn run() {{}}\n"),
            3,
        );
    }
}

#[test]
fn an_indented_code_block_is_rejected_at_its_first_line() {
    assert_doctest_at(
        "/// Parses.\n///\n///     let parsed = parse(\"x\");\n///     assert!(parsed.is_ok());\npub fn parse() {}\n",
        3,
    );
}

#[test]
fn a_doc_mixing_comments_and_doc_strings_unindents_as_rustdoc_does() {
    assert_doctest_at(
        "/// Seam.\n///\n#[doc = \"    let x = 1;\"]\npub fn seam() {}\n",
        3,
    );
    assert_clean("///    let x = 1;\n#[doc = \"Prose.\"]\npub fn seam() {}\n");
}

#[test]
fn a_doc_include_str_is_rejected_at_its_line() {
    for (text, line) in [
        ("#![doc = include_str!(\"../README.md\")]\n", 1),
        ("#[doc = include_str!(\"seam.md\")]\npub struct Seam;\n", 1),
        (
            "#[cfg_attr(docsrs, doc = include_str!(\"seam.md\"))]\npub struct Seam;\n",
            1,
        ),
        (
            "/// Seam.\n#[doc = include_str!(\"seam.md\")]\npub struct Seam;\n",
            2,
        ),
    ] {
        assert_doctest_at(text, line);
    }
}

#[test]
fn text_json_and_toml_fences_pass() {
    for tag in ["text", "json", "toml", "lua", "sh", "text,no_run"] {
        assert_clean(&format!(
            "/// Shape.\n///\n/// ```{tag}\n/// {{ \"a\": 1 }}\n/// ```\npub struct Shape;\n"
        ));
    }
}

#[test]
fn doc_text_rustdoc_never_compiles_passes() {
    assert_clean(
        r##"//! Crate docs that say `let x = 1;` inline.

///     Indented prose that rustdoc unindents to a paragraph.
///     More of it.
pub fn unindented() {}

/// - A list item
///
///   with a continuation paragraph.
pub fn listed() {}

#[doc(alias = "seam")]
#[doc = concat!("The [`", stringify!(Seam), "`] boundary.")]
pub struct Seam;

pub const FIXTURE: &str = r#"/// ```
/// let x = 1;
/// ```
"#;

macro_rules! documented {
    ($doc:expr, $name:ident) => {
        #[doc = $doc]
        pub struct $name;
    };
}
"##,
    );
}

#[test]
fn a_block_split_by_other_attributes_is_one_doc_reported_once() {
    assert_doctest_at(
        "/// ```\n#[must_use]\n/// let x = 1;\n/// ```\npub fn seam() -> u8 { 0 }\n",
        1,
    );
}

#[test]
fn every_compiled_block_in_one_doc_is_reported_at_its_line() {
    let violations = check(
        "/// One.\n///\n/// ```\n/// one();\n/// ```\n///\n/// ```text\n/// prose\n/// ```\n///\n/// ```no_run\n/// two();\n/// ```\npub fn both() {}\n",
    );
    let lines: Vec<_> = violations
        .iter()
        .map(|violation| violation.split(':').nth(1).unwrap_or_default())
        .collect();
    assert_eq!(lines, ["3", "11"], "{violations:?}");
}

#[test]
fn a_doc_comment_in_a_macro_body_is_rejected_at_its_line() {
    assert_doctest_at(
        "macro_rules! seam {\n    () => {\n        /// ```\n        /// seam!();\n        /// ```\n        pub struct Seam;\n    };\n}\n",
        3,
    );
}

#[test]
fn a_violation_names_the_file_line_rule_required_and_found() {
    let violations =
        check("/// Seam.\n///\n/// ```no_run\n/// seam();\n/// ```\npub fn seam() {}\n");
    assert_eq!(
        violations,
        [format!(
            "lib.rs:3: {RULE}: required {REQUIRED}, found `/// ```no_run`"
        )]
    );
}

#[test]
fn every_rs_file_in_every_workspace_crate_is_scanned() {
    let doctests = [
        "crates/tool/src/lib.rs",
        "crates/tool/src/main.rs",
        "crates/tool/src/bin/extra.rs",
        "crates/tool/build.rs",
        "crates/tool/tests/it/main.rs",
        "crates/tool/benches/bench.rs",
        "crates/tool/examples/demo.rs",
        "crates/gateway/stt/api/src/lib.rs",
    ];
    let mut files = vec![
        ("crates/tool/Cargo.toml", MANIFEST),
        ("crates/gateway/stt/api/Cargo.toml", MANIFEST),
    ];
    files.extend(doctests.iter().map(|path| (*path, DOCTEST)));
    let root = tree(&files);
    let violations = no_doctests_violations(root.path());
    assert_eq!(violations.len(), doctests.len(), "{violations:?}");
    for path in doctests {
        let file = at(root.path(), path);
        assert!(
            violations
                .iter()
                .any(|violation| violation.starts_with(&format!("{}:3: ", file.display()))),
            "{path} was not scanned: {violations:?}"
        );
    }
}

#[test]
fn a_crate_whose_manifest_does_not_parse_is_still_scanned() {
    let root = tree(&[
        ("crates/tool/Cargo.toml", b"[package\n"),
        ("crates/tool/src/lib.rs", DOCTEST),
    ]);
    let violations = no_doctests_violations(root.path());
    let file = at(root.path(), "crates/tool/src/lib.rs");
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(&format!("{}:3: ", file.display())),
        "{violations:?}"
    );
}

#[test]
fn sources_outside_every_crate_are_not_scanned() {
    let root = tree(&[
        ("crates/tool/Cargo.toml", MANIFEST),
        ("crates/tool/src/lib.rs", b"pub struct Plain;\n"),
        ("crates/tool/target/debug/build/out.rs", DOCTEST),
        ("crates/container/stray.rs", DOCTEST),
        ("crates/tool/src/notes.md", DOCTEST),
    ]);
    let violations = no_doctests_violations(root.path());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_workspace_with_no_rust_source_fails() {
    for files in [
        &[][..],
        &[("crates/tool/Cargo.toml", MANIFEST)][..],
        &[
            ("crates/tool/Cargo.toml", MANIFEST),
            ("crates/tool/README.md", DOCTEST),
        ][..],
    ] {
        let root = tree(files);
        let violations = no_doctests_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("scanned nothing"), "{violations:?}");
    }
}

#[test]
fn an_unreadable_or_unparseable_source_is_reported_not_skipped() {
    let unreadable: &[u8] = &[0xff, 0xfe, 0xfd];
    for (text, problem) in [
        (unreadable, ": unreadable source: "),
        (
            b"pub struct Seam;\npub use = Seam;\n".as_slice(),
            ":2: unparseable source: ",
        ),
        (b"pub struct Seam {\n".as_slice(), ": unparseable source: "),
    ] {
        let path = "crates/tool/src/broken.rs";
        let root = tree(&[("crates/tool/Cargo.toml", MANIFEST), (path, text)]);
        let file = at(root.path(), path);
        let violations = no_doctests_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with(&file.display().to_string())
                && violations[0].contains(problem),
            "{violations:?}"
        );
    }
}
