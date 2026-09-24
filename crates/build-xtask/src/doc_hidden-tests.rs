//! The `doc(hidden)` ban over this workspace's engine crates, plus
//! fixtures that each hiding form fails at its line and each doc form
//! that hides nothing passes.

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

/// Asserts `text` has exactly one violation, a hidden attribute at `line`.
fn assert_hidden_at(text: &str, line: usize) {
    let violations = check(text);
    assert_eq!(violations.len(), 1, "{text}\n{violations:?}");
    assert!(
        violations[0].starts_with(&format!("lib.rs:{line}: {RULE}: required ")),
        "{text}\n{violations:?}"
    );
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

const HIDDEN: &[u8] = b"#[doc(hidden)]\npub struct Seam;\n";
const FACADE_LIB: (&str, &[u8]) = ("crates/promptforge/src/lib.rs", b"pub use x::Y;\n");
const CONTAINER_LIB: (&str, &[u8]) = (
    "crates/promptforge-internal/lua/src/lib.rs",
    b"pub struct Plain;\n",
);

#[test]
fn the_real_engine_crates_carry_no_doc_hidden() {
    let violations = doc_hidden_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "doc(hidden) violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn a_hidden_item_of_every_kind_is_rejected_at_its_line() {
    for text in [
        "#[doc(hidden)]\npub struct Seam;",
        "#[doc(hidden)]\npub enum Seam {}",
        "#[doc(hidden)]\npub union Seam { a: u32 }",
        "#[doc(hidden)]\npub fn seam() {}",
        "#[doc(hidden)]\npub trait Seam {}",
        "#[doc(hidden)]\npub mod seam {}",
        "#[doc(hidden)]\n#[path = \"seam.rs\"]\npub mod seam;",
        "#[doc(hidden)]\npub const SEAM: u32 = 1;",
        "#[doc(hidden)]\npub static SEAM: u32 = 1;",
        "#[doc(hidden)]\npub type Seam = u32;",
        "#[doc(hidden)]\nimpl Seam {}",
        "#[doc(hidden)]\n#[macro_export]\nmacro_rules! seam { () => {} }",
    ] {
        assert_hidden_at(text, 1);
    }
}

#[test]
fn a_hidden_field_variant_or_method_is_rejected_at_its_line() {
    for (text, line) in [
        (
            "pub struct Wire {\n    #[doc(hidden)]\n    pub name: String,\n}",
            2,
        ),
        ("pub struct Wire(\n    #[doc(hidden)] pub String,\n);", 2),
        (
            "pub enum Kind {\n    Plain,\n    #[doc(hidden)]\n    Seam,\n}",
            3,
        ),
        (
            "pub enum Kind {\n    Plain {\n        #[doc(hidden)]\n        raw: u8,\n    },\n}",
            3,
        ),
        (
            "impl Wire {\n    #[doc(hidden)]\n    pub fn for_test() {}\n}",
            2,
        ),
        (
            "impl Wire {\n    #[doc(hidden)]\n    pub const SEAM: u8 = 0;\n}",
            2,
        ),
        (
            "pub trait Host {\n    #[doc(hidden)]\n    fn seam(&self);\n}",
            2,
        ),
        ("pub trait Host {\n    #[doc(hidden)]\n    type Seam;\n}", 2),
        (
            "unsafe extern \"C\" {\n    #[doc(hidden)]\n    pub fn seam();\n}",
            2,
        ),
    ] {
        assert_hidden_at(text, line);
    }
}

#[test]
fn a_hidden_re_export_is_rejected_at_its_line() {
    for (text, line) in [
        ("#[doc(hidden)]\npub use crate::error::Error;", 1),
        (
            "#[doc(hidden)]\npub use read::{ChunkSource, read_body_capped};",
            1,
        ),
        (
            "/// Seams.\npub mod model {\n    #[doc(hidden)]\n    pub use engine::Seam;\n}",
            3,
        ),
        ("#[doc(hidden)]\npub extern crate alloc;", 1),
    ] {
        assert_hidden_at(text, line);
    }
}

#[test]
fn an_inner_doc_hidden_is_rejected_at_its_line() {
    for (text, line) in [
        ("#![doc(hidden)]\n\npub struct Seam;", 1),
        ("pub mod seam {\n    #![doc(hidden)]\n}", 2),
        ("pub fn seam() {\n    #![doc(hidden)]\n}", 2),
    ] {
        assert_hidden_at(text, line);
    }
}

#[test]
fn hidden_in_a_doc_list_under_cfg_attr_or_spaced_out_is_rejected() {
    for text in [
        "#[doc(hidden, alias = \"seam\")]\npub struct Seam;",
        "#[doc(alias = \"seam\", hidden)]\npub struct Seam;",
        "#[cfg_attr(test, doc(hidden))]\npub struct Seam;",
        "#[cfg_attr(feature = \"test-support\", allow(dead_code), doc(hidden))]\npub struct Seam;",
        "#[cfg_attr(test, cfg_attr(unix, doc(hidden)))]\npub struct Seam;",
        "# [ doc ( hidden ) ]\npub struct Seam;",
    ] {
        assert_hidden_at(text, 1);
    }
}

#[test]
fn a_doc_hidden_in_a_macro_body_or_macro_input_is_rejected_at_its_line() {
    for (text, line) in [
        (
            "macro_rules! seams {\n    () => {\n        #[doc(hidden)]\n        pub struct Seam;\n    };\n}",
            3,
        ),
        (
            "events! {\n    pub enum Event {\n        #[doc(hidden)]\n        Seam {},\n    }\n}",
            3,
        ),
    ] {
        assert_hidden_at(text, line);
    }
}

#[test]
fn doc_forms_that_hide_nothing_pass() {
    let text = r##"//! Crate docs that mention `#[doc(hidden)]` in prose.
#![doc = include_str!("lib.md")]

/// Docs that say `#[doc(hidden)]` is gone.
#[doc(inline)]
pub use engine::Run;

#[doc = "hidden"]
#[doc(alias = "hidden")]
#[cfg_attr(test, doc(inline))]
#[cfg_attr(hidden, allow(dead_code))]
#[allow(hidden)]
pub struct Visible {
    /// A field.
    pub field: &'static str,
}

pub const TEXT: &str = "#[doc(hidden)]";
pub const RAW: &str = r#"#[doc(hidden)]"#;

macro_rules! forward {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        pub struct $name;
    };
}
"##;
    let violations = check(text);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_violation_names_the_file_line_rule_required_and_found() {
    let violations = check("/// A seam.\n#[doc(hidden)]\npub struct Seam;\n");
    assert_eq!(
        violations,
        [format!(
            "lib.rs:2: {RULE}: required {REQUIRED}, found `#[doc(hidden)]`"
        )]
    );
}

#[test]
fn every_rs_file_under_the_facade_and_the_container_is_scanned() {
    let hidden = [
        "crates/promptforge/tests/suite/main.rs",
        "crates/promptforge/benches/surface.rs",
        "crates/promptforge-internal/lua/src/nested/deep.rs",
        "crates/promptforge-internal/lua/build.rs",
        "crates/promptforge-internal/model-client/src/client/wire.rs",
        "crates/promptforge-internal/stray.rs",
    ];
    let mut files = vec![FACADE_LIB, CONTAINER_LIB];
    files.extend(hidden.iter().map(|path| (*path, HIDDEN)));
    let root = tree(&files);
    let violations = doc_hidden_violations(root.path());
    assert_eq!(violations.len(), hidden.len(), "{violations:?}");
    for path in hidden {
        let file = at(root.path(), path);
        assert!(
            violations
                .iter()
                .any(|violation| violation.starts_with(&format!("{}:1: ", file.display()))),
            "{path} was not scanned: {violations:?}"
        );
    }
}

#[test]
fn files_outside_the_engine_crates_are_not_scanned() {
    let root = tree(&[
        FACADE_LIB,
        CONTAINER_LIB,
        ("crates/workshop/server/src/lib.rs", HIDDEN),
        ("crates/gateway-api-discovery/src/lib.rs", HIDDEN),
        ("crates/promptforge-internal-old/src/lib.rs", HIDDEN),
        ("crates/promptforge-internal/target/debug/out.rs", HIDDEN),
        ("crates/promptforge/src/notes.md", HIDDEN),
    ]);
    let violations = doc_hidden_violations(root.path());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_missing_engine_directory_is_reported_not_skipped() {
    for (present, missing) in [
        (FACADE_LIB, "crates/promptforge-internal"),
        (CONTAINER_LIB, "crates/promptforge"),
    ] {
        let root = tree(&[present]);
        let violations = doc_hidden_violations(root.path());
        let dir = at(root.path(), missing);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with(&format!("{}: ", dir.display())),
            "{violations:?}"
        );
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
        let path = "crates/promptforge-internal/lua/src/broken.rs";
        let root = tree(&[FACADE_LIB, CONTAINER_LIB, (path, text)]);
        let file = at(root.path(), path);
        let violations = doc_hidden_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with(&file.display().to_string())
                && violations[0].contains(problem),
            "{violations:?}"
        );
    }
}
