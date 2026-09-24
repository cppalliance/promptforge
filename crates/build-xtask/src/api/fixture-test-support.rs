//! Generated workspaces for the `cargo xtask api` fixtures: a facade
//! crate over one internal crate, `promptforge-inner`, plus a stand-in
//! `serde` so the allowlist is exercised without a registry; one fixture
//! adds a second internal crate, `promptforge-outer`, downstream of the
//! first. Building their rustdoc JSON needs the pinned nightly, so every
//! test that calls [`findings`] or `execute` carries `#[ignore = "needs
//! the pinned nightly"]`.

use std::collections::BTreeSet;
use std::path::Path;

const WORKSPACE: &str = "[workspace]\nresolver = \"3\"\nmembers = [\
    \"crates/promptforge\", \"crates/promptforge-internal/*\", \"crates/serde\"]\n";

const FACADE_MANIFEST: &str = "[package]\nname = \"promptforge\"\nversion = \"0.0.0\"\n\
    edition = \"2024\"\npublish = false\n\n\
    [features]\ntest-support = [\"promptforge-inner/test-support\"]\n\n\
    [dependencies]\npromptforge-inner = { path = \"../promptforge-internal/inner\" }\n";

const OUTER_DEPENDENCY: &str = "promptforge-outer = { path = \"../promptforge-internal/outer\" }\n";

const INNER_MANIFEST: &str = "[package]\nname = \"promptforge-inner\"\nversion = \"0.0.0\"\n\
    edition = \"2024\"\npublish = false\n\n\
    [dependencies]\nserde = { path = \"../../serde\" }\n\n\
    [features]\ntest-support = []\n";

const OUTER_MANIFEST: &str = "[package]\nname = \"promptforge-outer\"\nversion = \"0.0.0\"\n\
    edition = \"2024\"\npublish = false\n\n\
    [dependencies]\npromptforge-inner = { path = \"../inner\" }\n";

const SERDE_MANIFEST: &str =
    "[package]\nname = \"serde\"\nversion = \"1.0.0\"\nedition = \"2024\"\npublish = false\n";

const SERDE_LIB: &str = "//! Stand-in for the allowlisted `serde`.\n\n\
    /// A stand-in serialization trait.\npub trait Serialize {}\n";

/// A workspace whose internal crate's `lib.rs` is `inner` and whose
/// facade's is `facade`, with its lockfile written so `--locked` holds.
pub(crate) fn workspace(inner: &str, facade: &str) -> tempfile::TempDir {
    generate(inner, None, facade)
}

/// [`workspace`] plus `promptforge-outer`, whose `lib.rs` is `outer`,
/// depending on the inner crate; the facade depends on both.
pub(crate) fn workspace_with_outer(inner: &str, outer: &str, facade: &str) -> tempfile::TempDir {
    generate(inner, Some(outer), facade)
}

fn generate(inner: &str, outer: Option<&str>, facade: &str) -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    let facade_manifest = match outer {
        Some(_) => format!("{FACADE_MANIFEST}{OUTER_DEPENDENCY}"),
        None => FACADE_MANIFEST.to_owned(),
    };
    for (path, text) in [
        ("Cargo.toml", WORKSPACE),
        ("crates/promptforge/Cargo.toml", &facade_manifest),
        ("crates/promptforge/src/lib.rs", facade),
        (
            "crates/promptforge-internal/inner/Cargo.toml",
            INNER_MANIFEST,
        ),
        ("crates/promptforge-internal/inner/src/lib.rs", inner),
        ("crates/serde/Cargo.toml", SERDE_MANIFEST),
        ("crates/serde/src/lib.rs", SERDE_LIB),
    ] {
        write(root.path(), path, text);
    }
    if let Some(outer) = outer {
        write(
            root.path(),
            "crates/promptforge-internal/outer/Cargo.toml",
            OUTER_MANIFEST,
        );
        write(
            root.path(),
            "crates/promptforge-internal/outer/src/lib.rs",
            outer,
        );
    }
    let output = super::load::cargo(root.path())
        .args(["generate-lockfile", "--offline"])
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "the fixture lockfile generates:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    root
}

/// Writes `text` at `path` under `root`, creating directories.
pub(crate) fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("a fixture file has a directory"))
        .expect("the fixture directory creates");
    std::fs::write(file, text).expect("the fixture file writes");
}

/// The findings for the workspace at `root`, as printed.
pub(crate) fn findings(root: &Path) -> BTreeSet<String> {
    super::report(root)
        .expect("the fixture documents")
        .findings
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The findings `expected` names, as a set to compare with [`findings`].
pub(crate) fn set<const N: usize>(expected: [String; N]) -> BTreeSet<String> {
    expected.into_iter().collect()
}

/// A closure finding's text after the mention, for an item of internal
/// crate `krate` the fixture facade does not re-export.
pub(crate) fn leak(krate: &str) -> String {
    format!(
        "required {}, found an item of internal crate `{krate}` the facade does not re-export",
        super::items::required()
    )
}
