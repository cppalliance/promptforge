//! The facade shape check over this workspace's facade, plus fixtures that
//! each allowed form passes and each forbidden form fails at its line.

use std::path::{Path, PathBuf};

use super::*;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// The dependency crates the source fixtures may re-export from.
const FIXTURE_CRATES: [&str; 3] = [
    "promptforge_engine",
    "promptforge_model_client",
    "promptforge_types",
];

/// A facade manifest declaring exactly [`FIXTURE_CRATES`].
const FIXTURE_MANIFEST: &str = "[package]\nname = \"promptforge\"\n\n[dependencies]\n\
    promptforge-engine.workspace = true\n\
    promptforge-model-client.workspace = true\n\
    promptforge-types.workspace = true\n";

fn check(text: &str) -> Vec<String> {
    let crates = FIXTURE_CRATES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    source_violations(Path::new("lib.rs"), text, &crates)
}

/// Asserts `text` has exactly one violation, reported at `line` for `rule`.
fn assert_one(text: &str, line: usize, rule: &str) {
    let violations = check(text);
    assert_eq!(violations.len(), 1, "{text}\n{violations:?}");
    assert!(
        violations[0].starts_with(&format!("lib.rs:{line}: {rule}: required ")),
        "{text}\n{violations:?}"
    );
}

fn facade_dir(root: &Path) -> PathBuf {
    root.join("crates").join("promptforge")
}

/// Writes `manifest`, when given, as a fake workspace's
/// `crates/promptforge/Cargo.toml`, and each `(path, text)` under its `src/`.
fn facade_root(manifest: Option<&str>, files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    let dir = facade_dir(root.path());
    std::fs::create_dir_all(&dir).expect("the facade directory creates");
    if let Some(manifest) = manifest {
        std::fs::write(dir.join("Cargo.toml"), manifest).expect("the manifest writes");
    }
    let src = dir.join("src");
    for (path, text) in files {
        let file = src.join(path);
        let dir = file.parent().expect("a source file has a directory");
        std::fs::create_dir_all(dir).expect("the source directory creates");
        std::fs::write(&file, text).expect("the source file writes");
    }
    root
}

#[test]
fn the_real_facade_passes_the_shape_check() {
    let violations = facade_shape_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "facade shape violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_allowed_form_passes() {
    let text = r#"#![doc = include_str!("lib.md")]

pub use promptforge_engine::Run;
pub use promptforge_model_client::Error as ClientError;
pub use ::promptforge_engine::Step;

/// The effects a run issues.
pub mod effect {
    //! More on effects.

    /// A re-export may carry its own docs.
    #[doc(inline)]
    pub use promptforge_engine::Effect;

    /// A grouping module may nest.
    pub mod record {
        pub use promptforge_engine::EffectRecord;
    }
}

/// A grouping module may live in its own file.
pub mod event;

/// The engine's test drivers.
#[cfg(feature = "test-support")]
pub mod test_support {
    #[cfg(feature = "test-support")]
    pub use promptforge_engine::test_support::drive_tokio;
}
"#;
    let violations = check(text);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_violation_names_the_file_line_rule_required_and_found() {
    let violations =
        check("/// Effects.\npub mod effect {\n    pub use promptforge_engine::*;\n}\n");
    assert_eq!(
        violations,
        [format!(
            "lib.rs:3: glob re-export: required {REEXPORT}, found `pub use promptforge_engine::*;`"
        )]
    );
}

#[test]
fn glob_grouped_crate_and_facade_path_re_exports_are_rejected() {
    for (text, rule) in [
        ("pub use promptforge_engine::*;", "glob re-export"),
        ("pub use promptforge_engine::input::*;", "glob re-export"),
        (
            "pub use promptforge_engine::{Run, Step};",
            "grouped use list",
        ),
        ("pub use promptforge_engine::{Run};", "grouped use list"),
        ("pub use promptforge_engine;", "crate re-export"),
        ("pub use promptforge_engine as engine;", "crate re-export"),
        ("pub use ::promptforge_engine;", "crate re-export"),
        (
            "pub use crate::effect::Effect;",
            "re-export of a facade path",
        ),
        (
            "pub use self::effect::Effect;",
            "re-export of a facade path",
        ),
        ("pub use super::Run;", "re-export of a facade path"),
        ("use promptforge_engine::Run;", "non-public use"),
        ("pub(crate) use promptforge_engine::Run;", "non-public use"),
    ] {
        assert_one(text, 1, rule);
    }
}

#[test]
fn a_re_export_rooted_outside_the_facade_dependencies_is_rejected() {
    const RULE: &str = "re-export not rooted at a facade dependency";
    for (text, line) in [
        ("pub use effect::Effect;", 1),
        (
            "/// Effects.\npub mod effect {\n    pub use record::EffectRecord;\n}\n",
            3,
        ),
        ("pub use std::fmt::Debug;", 1),
        ("pub use ::serde::Serialize;", 1),
        ("pub use promptforge_lua::StoreOp;", 1),
    ] {
        assert_one(text, line, RULE);
    }
}

#[test]
fn the_dependency_crates_are_the_manifest_dependency_keys() {
    let manifest = "[package]\nname = \"promptforge\"\n\n\
        [dependencies]\n\
        promptforge-engine.workspace = true\n\
        types = { package = \"promptforge-types\", workspace = true }\n\n\
        [target.'cfg(windows)'.dependencies]\n\
        promptforge-vfs.workspace = true\n\n\
        [dev-dependencies]\n\
        serde_json.workspace = true\n";
    let lib = "pub use promptforge_engine::Run;\n\
        pub use types::event::Event;\n\
        pub use promptforge_vfs::Vfs;\n\
        pub use serde_json::Value;\n\
        pub use promptforge_types::ids::RunId;\n";
    let root = facade_root(Some(manifest), &[("lib.rs", lib)]);
    let violations = facade_shape_violations(root.path());
    assert_eq!(violations.len(), 2, "{violations:?}");
    for (violation, line) in violations.iter().zip([4, 5]) {
        assert!(
            violation.contains(&format!(
                "lib.rs:{line}: re-export not rooted at a facade dependency: "
            )),
            "{violations:?}"
        );
    }
}

#[test]
fn every_item_definition_is_rejected_with_its_kind() {
    for (text, kind) in [
        ("pub fn run() {}", "fn"),
        ("pub struct Run;", "struct"),
        ("pub enum Step {}", "enum"),
        ("pub trait Host {}", "trait"),
        ("impl Clone for Run {}", "impl"),
        ("pub const LIMIT: u32 = 1;", "const"),
        ("pub static NAME: &str = \"\";", "static"),
        ("pub type Result = ();", "type alias"),
        ("macro_rules! forward { () => {} }", "macro_rules!"),
        ("pub union Bits { a: u32 }", "union"),
        ("extern crate promptforge_engine;", "extern crate"),
        ("extern \"C\" {}", "extern block"),
        ("include!(\"items.rs\");", "macro invocation"),
    ] {
        assert_one(text, 1, &format!("item definition ({kind})"));
    }
}

#[test]
fn an_item_definition_inside_a_grouping_module_is_rejected() {
    assert_one(
        "/// Effects.\npub mod effect {\n    /// Hidden.\n    pub struct Hidden;\n}\n",
        4,
        "item definition (struct)",
    );
}

#[test]
fn attributes_other_than_doc_and_the_test_support_cfg_are_rejected() {
    for (text, line) in [
        ("#![forbid(unsafe_code)]\n", 1),
        (
            "#[allow(unused_imports)]\npub use promptforge_engine::Run;",
            1,
        ),
        ("#[cfg(test)]\npub use promptforge_engine::Run;", 1),
        (
            "#[cfg(feature = \"other\")]\npub use promptforge_engine::Run;",
            1,
        ),
        (
            "#[cfg(not(feature = \"test-support\"))]\npub use promptforge_engine::Run;",
            1,
        ),
        ("/// Effects.\n#[path = \"effects.rs\"]\npub mod effect;", 2),
    ] {
        assert_one(text, line, "disallowed attribute");
    }
}

#[test]
fn a_module_must_be_public_and_carry_a_doc_comment() {
    for (text, line, rule) in [
        ("/// Effects.\nmod effect {}", 2, "non-public module"),
        (
            "/// Effects.\npub(crate) mod effect {}",
            2,
            "non-public module",
        ),
        ("pub mod effect {}", 1, "undocumented module"),
        ("pub mod effect;", 1, "undocumented module"),
        (
            "#[doc(hidden)]\npub mod effect {}",
            2,
            "undocumented module",
        ),
    ] {
        assert_one(text, line, rule);
    }
}

#[test]
fn unparseable_source_is_reported_at_its_line() {
    assert_one(
        "pub use promptforge_engine::Run;\npub use = Step;\n",
        2,
        "unparseable source",
    );
}

#[test]
fn every_source_file_under_the_facade_src_is_checked() {
    let root = facade_root(
        Some(FIXTURE_MANIFEST),
        &[
            ("lib.rs", "/// Events.\npub mod event;\n"),
            ("event.rs", "pub use promptforge_types::event::Event;\n"),
            ("nested/stray.rs", "pub struct Stray;\n"),
        ],
    );
    let violations = facade_shape_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("stray.rs:1: item definition (struct)"),
        "{violations:?}"
    );
}

#[test]
fn a_missing_facade_crate_root_is_reported_not_skipped() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let lib = root
        .path()
        .join("crates")
        .join("promptforge")
        .join("src")
        .join("lib.rs");
    let violations = facade_shape_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(&lib.display().to_string()),
        "{violations:?}"
    );
}

#[test]
fn an_unreadable_facade_source_is_reported_not_skipped() {
    let root = facade_root(
        Some(FIXTURE_MANIFEST),
        &[("lib.rs", "/// Events.\npub mod event;\n")],
    );
    let event = facade_dir(root.path()).join("src").join("event.rs");
    std::fs::write(&event, [0xff, 0xfe, 0xfd]).expect("the non-UTF-8 source writes");
    let violations = facade_shape_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(&format!("{}: unreadable facade source: ", event.display())),
        "{violations:?}"
    );
}

#[test]
fn a_missing_or_unparseable_facade_manifest_is_reported_not_skipped() {
    for (manifest, problem) in [
        (None, "unreadable facade manifest"),
        (Some("[dependencies\n"), "unparseable facade manifest"),
    ] {
        let root = facade_root(manifest, &[("lib.rs", "pub struct Stray;\n")]);
        let path = facade_dir(root.path()).join("Cargo.toml");
        let violations = facade_shape_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with(&format!("{}: {problem}: ", path.display())),
            "{violations:?}"
        );
    }
}
