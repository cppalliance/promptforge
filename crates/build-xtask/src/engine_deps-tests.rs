//! Fixture tests for the Engine dependency guard: manifest cases write one
//! manifest into a temporary directory and scan it in isolation; graph
//! cases write a small workspace of local path crates and resolve it.

use std::path::{Path, PathBuf};

use super::*;

/// Writes one manifest into a fresh temporary directory and returns its path
/// beside the directory guard that keeps it alive.
fn manifest(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("Cargo.toml");
    std::fs::write(&path, format!("[package]\nname = \"fixture\"\n{text}"))
        .expect("the manifest writes");
    (dir, path)
}

#[test]
fn a_clean_engine_manifest_has_no_violations() {
    let (_dir, path) = manifest(
        "[dependencies]\nserde = \"1\"\nmlua = { version = \"0.10\", features = [\"lua54\"] }\n\
         [build-dependencies]\ncc = \"1\"\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_forbidden_crate_in_dependencies_is_reported() {
    let (_dir, path) =
        manifest("[dependencies]\ntokio = { version = \"1\", features = [\"rt\"] }\n");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let rendered = violations[0].to_string();
    assert!(
        rendered.contains("[dependencies]") && rendered.contains("tokio"),
        "the violation names the table and the crate: {rendered}"
    );
}

#[test]
fn a_forbidden_crate_in_dev_dependencies_is_reported() {
    let (_dir, path) =
        manifest("[dev-dependencies]\ntokio = { version = \"1\", features = [\"test-util\"] }\n");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let rendered = violations[0].to_string();
    assert!(
        rendered.contains("[dev-dependencies]") && rendered.contains("tokio"),
        "tests are Engine code too, so dev-dependencies are scanned: {rendered}"
    );
}

#[test]
fn tokio_in_a_target_specific_dev_table_is_reported() {
    let (_dir, path) = manifest("[target.'cfg(windows)'.dev-dependencies]\ntokio = \"1\"\n");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let rendered = violations[0].to_string();
    assert!(
        rendered.contains("[target.'cfg(windows)'.dev-dependencies]") && rendered.contains("tokio"),
        "the violation names the target-specific dev table: {rendered}"
    );
}

#[test]
fn an_optional_entry_enabled_only_by_test_support_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(
        violations.len(),
        1,
        "no feature exempts a forbidden entry: {violations:?}"
    );
}

#[test]
fn an_optional_entry_enabled_by_another_feature_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\nreqwest = { version = \"0.12\", optional = true }\n\
         [features]\ntest-support = [\"dep:reqwest\"]\nnet = [\"dep:reqwest\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(
        violations.len(),
        1,
        "an optional entry is reported once, however many features enable it: {violations:?}"
    );
}

#[test]
fn a_dep_gated_entry_also_enabled_by_a_strong_feature_path_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\nrt = [\"tokio/rt\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn a_dep_gated_entry_with_only_a_weak_feature_path_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\nrt = [\"tokio?/rt\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn an_entry_whose_test_support_gate_is_enabled_by_default_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ndefault = [\"test-support\"]\ntest-support = [\"dep:tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn an_optional_entry_with_an_implicit_feature_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn build_and_target_specific_tables_are_scanned_and_renames_resolved() {
    let (_dir, path) = manifest(
        "[build-dependencies]\nasync-trait = \"0.1\"\n\
         [target.'cfg(windows)'.dependencies]\ntu = { package = \"tokio-util\", version = \"0.7\" }\n\
         [target.'cfg(unix)'.dev-dependencies]\ntokio = \"1\"\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 3, "{violations:?}");
    let rendered: Vec<String> = violations.iter().map(ToString::to_string).collect();
    assert!(
        rendered
            .iter()
            .any(|v| v.contains("[build-dependencies]") && v.contains("async-trait")),
        "the build-dependencies entry is reported: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|v| v.contains("cfg(windows)") && v.contains("tokio-util")),
        "the renamed target-specific entry is reported by package name: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|v| v.contains("cfg(unix)'.dev-dependencies") && v.contains("tokio")),
        "the target-specific dev entry is reported: {rendered:?}"
    );
}

#[test]
fn an_unreadable_or_unparseable_manifest_is_reported() {
    let (dir, path) = manifest("not [valid toml");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].to_string().contains("unparseable manifest"),
        "{violations:?}"
    );
    let missing = forbidden_engine_dependencies(&dir.path().join("absent").join("Cargo.toml"));
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].to_string().contains("unreadable manifest"),
        "{missing:?}"
    );
    for crate_dir in [dir.path().to_path_buf(), dir.path().join("absent")] {
        let graph = forbidden_engine_graph(dir.path(), &crate_dir);
        assert!(
            graph.is_empty(),
            "the manifest scan alone reports an unreadable manifest: {graph:?}"
        );
    }
    let unnamed = dir.path().join("unnamed");
    std::fs::create_dir_all(&unnamed).expect("the crate directory creates");
    std::fs::write(
        unnamed.join("Cargo.toml"),
        "[dependencies]\nserde = \"1\"\n",
    )
    .expect("the manifest writes");
    let declared = forbidden_engine_dependencies(&unnamed.join("Cargo.toml"));
    assert!(declared.is_empty(), "{declared:?}");
    let unchecked = forbidden_engine_graph(dir.path(), &unnamed);
    assert_eq!(unchecked.len(), 1, "{unchecked:?}");
    assert!(
        unchecked[0].to_string().contains("names no package"),
        "a manifest that names no package leaves its graph unchecked, so it is reported: {unchecked:?}"
    );
}

/// One local crate in a graph fixture: its directory under the fixture
/// root, its package name, and its manifest body after `[package]`.
type LocalCrate<'a> = (&'a str, &'a str, &'a str);

/// The local stand-ins for forbidden crates every graph fixture holds, so
/// `cargo tree` resolves offline. The `tempfile` stand-in depends on the
/// `getrandom` and `fastrand` ones, as the real crate does.
const STAND_INS: [LocalCrate<'static>; 4] = [
    ("vendor/tokio", "tokio", ""),
    ("vendor/getrandom", "getrandom", ""),
    ("vendor/fastrand", "fastrand", ""),
    (
        "vendor/tempfile",
        "tempfile",
        "[dependencies]\ngetrandom = { path = \"../getrandom\" }\n\
         fastrand = { path = \"../fastrand\" }\n",
    ),
];

/// Writes a fixture workspace whose members are the stand-ins and `crates`.
fn graph_workspace(crates: &[LocalCrate<'_>]) -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    let mut members = Vec::new();
    for (dir, name, body) in STAND_INS.iter().chain(crates) {
        let crate_dir = root.path().join(dir);
        std::fs::create_dir_all(crate_dir.join("src")).expect("the crate directory creates");
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{body}"
            ),
        )
        .expect("the manifest writes");
        std::fs::write(crate_dir.join("src").join("lib.rs"), "").expect("lib.rs writes");
        members.push(format!("\"{dir}\""));
    }
    std::fs::write(
        root.path().join("Cargo.toml"),
        format!(
            "[workspace]\nresolver = \"3\"\nmembers = [{}]\n",
            members.join(", ")
        ),
    )
    .expect("the workspace manifest writes");
    root
}

/// The rendered graph findings for the fixture crate at `dir`.
fn graph_findings(root: &Path, dir: &str) -> Vec<String> {
    forbidden_engine_graph(root, &root.join(dir))
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_forbidden_crate_reached_through_a_dev_dependency_is_reported() {
    let engine = "crates/runvil-internal/engine";
    let root = graph_workspace(&[
        (
            "vendor/driver",
            "driver",
            "[dependencies]\ntokio = { path = \"../tokio\" }\n",
        ),
        (
            engine,
            "runvil-engine",
            "[dev-dependencies]\ndriver = { path = \"../../../vendor/driver\" }\n",
        ),
    ]);
    let declared = forbidden_engine_dependencies(&root.path().join(engine).join("Cargo.toml"));
    assert!(
        declared.is_empty(),
        "the manifest names no forbidden crate, so only the graph can catch it: {declared:?}"
    );
    let found = graph_findings(root.path(), engine);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("runvil-engine")
            && found[0].contains("resolved graph")
            && found[0].contains("tokio"),
        "the finding names the Engine crate, the graph, and the package: {found:?}"
    );
}

#[test]
fn a_forbidden_crate_reached_only_on_another_platform_is_reported() {
    let engine = "crates/runvil-internal/engine";
    let root = graph_workspace(&[
        (
            "vendor/driver",
            "driver",
            "[target.wasm32-unknown-unknown.dependencies]\ntokio = { path = \"../tokio\" }\n",
        ),
        (
            engine,
            "runvil-engine",
            "[dependencies]\ndriver = { path = \"../../../vendor/driver\" }\n",
        ),
    ]);
    let declared = forbidden_engine_dependencies(&root.path().join(engine).join("Cargo.toml"));
    assert!(
        declared.is_empty(),
        "the manifest names no forbidden crate, so only the graph can catch it: {declared:?}"
    );
    let found = graph_findings(root.path(), engine);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("runvil-engine")
            && found[0].contains("resolved graph")
            && found[0].contains("tokio"),
        "a forbidden crate behind a non-host target edge is reported: {found:?}"
    );
}

#[test]
fn getrandom_reached_through_any_edge_is_reported() {
    let root = graph_workspace(&[
        (
            "vendor/seeder",
            "seeder",
            "[dependencies]\ngetrandom = { path = \"../getrandom\" }\n",
        ),
        (
            "crates/runvil-internal/normal",
            "runvil-normal",
            "[dependencies]\nseeder = { path = \"../../../vendor/seeder\" }\n",
        ),
        (
            "crates/runvil-internal/build",
            "runvil-build",
            "[build-dependencies]\nseeder = { path = \"../../../vendor/seeder\" }\n",
        ),
        (
            "crates/runvil-internal/dev",
            "runvil-dev",
            "[dev-dependencies]\nseeder = { path = \"../../../vendor/seeder\" }\n",
        ),
        // Only a graph resolved with all features reaches this optional entry.
        (
            "crates/runvil-internal/optional",
            "runvil-optional",
            "[dependencies]\nseeder = { path = \"../../../vendor/seeder\", optional = true }\n\
             [features]\nseed = [\"dep:seeder\"]\n",
        ),
    ]);
    for edge in ["normal", "build", "dev", "optional"] {
        let found = graph_findings(root.path(), &format!("crates/runvil-internal/{edge}"));
        assert_eq!(found.len(), 1, "{edge}: {found:?}");
        assert!(
            found[0].contains(&format!("runvil-{edge}")) && found[0].contains("getrandom"),
            "getrandom reached through the {edge} seeder entry is reported: {found:?}"
        );
    }
}

#[test]
fn a_tempfile_dev_dependency_is_reported_for_getrandom_and_fastrand() {
    let vfs = "crates/runvil-internal/vfs";
    let root = graph_workspace(&[(
        vfs,
        "runvil-vfs",
        "[dev-dependencies]\ntempfile = { path = \"../../../vendor/tempfile\" }\n",
    )]);
    let found = graph_findings(root.path(), vfs);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found[0].contains("fastrand") && found[1].contains("getrandom"),
        "each entropy crate is reported once, in name order: {found:?}"
    );
    assert!(
        found.iter().all(|f| !f.contains("reaches tempfile")),
        "tempfile itself is not forbidden: {found:?}"
    );
}

#[test]
fn a_clean_engine_crate_graph_has_no_violations() {
    // `driver` turns on `clean`'s optional tokio. Resolving the workspace as
    // a whole would unify that feature into the Engine crate's graph;
    // resolving the Engine crate alone does not.
    let engine = "crates/runvil-internal/engine";
    let root = graph_workspace(&[
        (
            "vendor/clean",
            "clean",
            "[dependencies]\ntokio = { path = \"../tokio\", optional = true }\n\
             [features]\nrt = [\"dep:tokio\"]\n",
        ),
        (
            "vendor/driver",
            "driver",
            "[dependencies]\nclean = { path = \"../clean\", features = [\"rt\"] }\n",
        ),
        (
            engine,
            "runvil-engine",
            "[dependencies]\nclean = { path = \"../../../vendor/clean\" }\n\
             [build-dependencies]\nclean = { path = \"../../../vendor/clean\" }\n\
             [dev-dependencies]\nclean = { path = \"../../../vendor/clean\" }\n",
        ),
    ]);
    let found = graph_findings(root.path(), engine);
    assert!(found.is_empty(), "{found:?}");
}
