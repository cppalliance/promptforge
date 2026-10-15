//! The Engine clippy guard over this workspace, plus fixtures that each
//! failure case is reported against a fake workspace that otherwise passes.

use std::path::Path;

use super::*;
use crate::product::test_support::{self, workspace_root};

/// Writes `text` at `path`, written with `/`, under `root`.
fn write(root: &Path, path: &str, text: &str) {
    let file = at(root, path);
    let dir = file.parent().expect("a fixture file has a directory");
    std::fs::create_dir_all(dir).expect("the fixture directory creates");
    std::fs::write(&file, text).expect("the fixture file writes");
}

/// A fake workspace that passes the guard: the root `clippy.toml`, the
/// three Engine files, both root Engine crates, and one container crate.
fn clean_root() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    test_support::write_engine_clippy_tomls(root.path());
    for (dir, package) in [
        ("promptforge", "promptforge"),
        ("promptforge-plugin", "promptforge-plugin"),
        ("promptforge-internal/engine", "promptforge-engine"),
    ] {
        test_support::write_crate(root.path(), dir, package, "");
        write(
            root.path(),
            &format!("crates/{dir}/src/lib.rs"),
            "pub struct Live;\n",
        );
    }
    root
}

/// Asserts the findings, in order: each starts with its file's path, and
/// any `:<line>:` written after it, and holds each of its fragments.
fn assert_findings(root: &Path, found: &[String], expected: &[(&str, &[&str])]) {
    assert_eq!(found.len(), expected.len(), "{found:#?}");
    for (finding, (path, fragments)) in found.iter().zip(expected) {
        let (file, line) = path.split_at(path.find(':').unwrap_or(path.len()));
        let path = format!("{}{line}", at(root, file).display());
        assert!(
            finding.starts_with(&path),
            "expected a finding for {path}: {finding}"
        );
        for fragment in *fragments {
            assert!(
                finding.contains(fragment),
                "expected `{fragment}` in: {finding}"
            );
        }
    }
}

#[test]
fn an_engine_crate_resolving_to_another_clippy_toml_is_reported() {
    let root = clean_root();
    test_support::write_crate(
        root.path(),
        "promptforge-internal/lua",
        "promptforge-lua",
        "",
    );
    write(
        root.path(),
        "crates/promptforge-internal/lua/clippy.toml",
        &test_support::engine_clippy_toml(None),
    );
    // Clippy reads `.clippy.toml` before `clippy.toml` in one directory.
    test_support::write_crate(
        root.path(),
        "promptforge-internal/vfs",
        "promptforge-vfs",
        "",
    );
    write(
        root.path(),
        "crates/promptforge-internal/vfs/.clippy.toml",
        &test_support::engine_clippy_toml(None),
    );
    write(
        root.path(),
        "crates/promptforge-internal/vfs/clippy.toml",
        &test_support::engine_clippy_toml(None),
    );
    let found = engine_clippy_violations(root.path());
    let lua_toml = at(root.path(), "crates/promptforge-internal/lua/clippy.toml");
    let vfs_toml = at(root.path(), "crates/promptforge-internal/vfs/.clippy.toml");
    assert_findings(
        root.path(),
        &found,
        &[
            (
                "crates/promptforge-internal/lua/Cargo.toml",
                &[
                    "required: the nearest clippy.toml is one of",
                    "crates/promptforge-internal/clippy.toml",
                    &format!("found: {}", lua_toml.display()),
                ],
            ),
            (
                "crates/promptforge-internal/vfs/Cargo.toml",
                &[&format!("found: {}", vfs_toml.display())],
            ),
        ],
    );

    let bare = clean_root();
    std::fs::remove_file(at(bare.path(), "clippy.toml")).expect("the root clippy.toml removes");
    std::fs::remove_file(at(bare.path(), "crates/promptforge-plugin/clippy.toml"))
        .expect("the plugin clippy.toml removes");
    let found = engine_clippy_violations(bare.path());
    assert!(
        found.iter().any(|f| f.starts_with(
            &at(bare.path(), "crates/promptforge-plugin/Cargo.toml")
                .display()
                .to_string()
        ) && f.contains("found: no clippy.toml")),
        "a crate with no clippy.toml at or below the workspace root is reported: {found:#?}"
    );
}

#[test]
fn a_clippy_toml_missing_a_root_entry_is_reported() {
    let root = clean_root();
    write(
        root.path(),
        "crates/promptforge-internal/clippy.toml",
        &test_support::engine_clippy_toml(Some("std::panic::set_hook")),
    );
    write(
        root.path(),
        "crates/promptforge-plugin/clippy.toml",
        &test_support::engine_clippy_toml(Some("allow-unwrap-in-tests")),
    );
    write(
        root.path(),
        "crates/promptforge/clippy.toml",
        &test_support::engine_clippy_toml(None)
            .replace(
                "allow-unwrap-in-tests = true",
                "allow-unwrap-in-tests = false",
            )
            .replace("use a returned error instead", "return an error"),
    );
    let found = engine_clippy_violations(root.path());
    assert_findings(
        root.path(),
        &found,
        &[
            (
                "crates/promptforge-internal/clippy.toml",
                &[
                    "required: the root clippy.toml's disallowed-methods entry",
                    "std::panic::set_hook",
                    "found: no entry",
                ],
            ),
            (
                "crates/promptforge/clippy.toml",
                &[
                    "required: the root clippy.toml's setting `allow-unwrap-in-tests = true`",
                    "found: `allow-unwrap-in-tests = false`",
                ],
            ),
            (
                "crates/promptforge/clippy.toml",
                &["std::panic::set_hook", "found:", "return an error"],
            ),
            (
                "crates/promptforge-plugin/clippy.toml",
                &[
                    "required: the root clippy.toml's setting `allow-unwrap-in-tests = true`",
                    "found: no `allow-unwrap-in-tests`",
                ],
            ),
        ],
    );

    let rootless = clean_root();
    std::fs::write(at(rootless.path(), "clippy.toml"), "not [valid toml")
        .expect("the root clippy.toml writes");
    let found = engine_clippy_violations(rootless.path());
    assert_findings(
        rootless.path(),
        &found,
        &[("clippy.toml", &["unparseable", "required:"])],
    );
}

#[test]
fn a_clippy_toml_missing_a_determinism_ban_is_reported() {
    let root = clean_root();
    write(
        root.path(),
        "crates/promptforge/clippy.toml",
        &test_support::engine_clippy_toml(Some("\"std::thread::park_timeout\"")),
    );
    write(
        root.path(),
        "crates/promptforge-plugin/clippy.toml",
        &test_support::engine_clippy_toml(None).replace(
            "{ path = \"std::thread::sleep\", reason = \"use the deterministic form instead\" }",
            "\"std::thread::sleep\"",
        ),
    );
    let found = engine_clippy_violations(root.path());
    assert_findings(
        root.path(),
        &found,
        &[
            (
                "crates/promptforge/clippy.toml",
                &[
                    "required: the determinism ban `std::thread::park_timeout` with a reason",
                    "found: no entry",
                ],
            ),
            (
                "crates/promptforge-plugin/clippy.toml",
                &["`std::thread::sleep`", "found: an entry with no reason"],
            ),
        ],
    );

    let missing = clean_root();
    std::fs::remove_file(at(
        missing.path(),
        "crates/promptforge-internal/clippy.toml",
    ))
    .expect("the internal clippy.toml removes");
    let found = engine_clippy_violations(missing.path());
    let internal = at(missing.path(), "crates/promptforge-internal/clippy.toml");
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(&internal.display().to_string()) && f.contains("unreadable")),
        "a missing Engine clippy.toml holds no ban, so it is reported: {found:#?}"
    );
}

#[test]
fn an_engine_source_suppressing_disallowed_methods_is_reported() {
    let root = clean_root();
    let engine = "crates/promptforge-internal/engine";
    write(
        root.path(),
        &format!("{engine}/src/lib.rs"),
        "//! Mentions clippy::disallowed_methods in prose.\n\
         #![expect(clippy::disallowed_methods, reason = \"crate-wide\")]\n\
         /// Never `#[allow(clippy::disallowed_methods)]`.\n\
         #[expect(clippy::unwrap_used, reason = \"another lint\")]\n\
         #[warn(clippy::disallowed_methods)]\n\
         pub struct Live;\n",
    );
    write(
        root.path(),
        &format!("{engine}/tests/it/main.rs"),
        "mod a {\n    fn b() {}\n\n    #[expect(\n        clippy::disallowed_methods,\n        \
         reason = \"multi-line\"\n    )]\n    fn c() {}\n}\n",
    );
    write(
        root.path(),
        &format!("{engine}/benches/bench.rs"),
        "#[cfg_attr(test, allow(clippy::disallowed_methods))]\nfn main() {}\n\
         #[allow(clippy::all)]\nfn helper() {}\n",
    );
    write(
        root.path(),
        &format!("{engine}/examples/example.rs"),
        "macro_rules! m {\n    () => {\n        #[expect(clippy::style, reason = \"its group\")]\n        \
         fn g() {}\n    };\n}\nfn main() {}\n",
    );
    // A crate outside the Engine may suppress the lint, as a binary entry
    // point installing process-global state does.
    test_support::write_crate(root.path(), "harness", "harness", "");
    write(
        root.path(),
        "crates/harness/src/main.rs",
        "#[expect(clippy::disallowed_methods, reason = \"entry point\")]\nfn main() {}\n",
    );
    let found = engine_clippy_violations(root.path());
    let required = "required: no allow or expect of clippy::disallowed_methods";
    assert_findings(
        root.path(),
        &found,
        &[
            (
                &format!("{engine}/benches/bench.rs:1:"),
                &[
                    required,
                    "found: `#[cfg_attr(test, allow(clippy::disallowed_methods))]`",
                ],
            ),
            (
                &format!("{engine}/benches/bench.rs:3:"),
                &[required, "found: `#[allow(clippy::all)]`"],
            ),
            (
                &format!("{engine}/examples/example.rs:3:"),
                &[required, "clippy::style"],
            ),
            (
                &format!("{engine}/src/lib.rs:2:"),
                &[required, "#![expect(clippy::disallowed_methods"],
            ),
            (
                &format!("{engine}/tests/it/main.rs:4:"),
                &[required, "found: `#[expect(`"],
            ),
        ],
    );
}

#[test]
fn the_engine_clippy_tomls_pass() {
    let found = engine_clippy_violations(&workspace_root());
    assert!(found.is_empty(), "{}", found.join("\n"));
    let clean = clean_root();
    let found = engine_clippy_violations(clean.path());
    assert!(
        found.is_empty(),
        "the fixture every failure case starts from passes: {found:#?}"
    );
}
