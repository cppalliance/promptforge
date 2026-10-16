//! Shared fixtures for the product-boundary test siblings and the Engine
//! guards.

use std::path::{Path, PathBuf};

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// Writes a minimal crate manifest into a fake workspace; `dir_name` may
/// contain a slash to nest the crate under a container
/// (`runvil-internal/lua`).
pub(crate) fn write_crate(root: &Path, dir_name: &str, package: &str, deps: &str) {
    let dir = root.join("crates").join(dir_name);
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\n{deps}"),
    )
    .expect("the manifest writes");
}

/// A fixture root `clippy.toml`: one setting and one `disallowed-methods`
/// entry for the Engine files to repeat.
pub(crate) const ROOT_CLIPPY_TOML: &str = "allow-unwrap-in-tests = true\n\
    disallowed-methods = [\n    \
    { path = \"std::panic::set_hook\", reason = \"use a returned error instead\" },\n\
    ]\n";

/// An Engine `clippy.toml` that repeats [`ROOT_CLIPPY_TOML`] and holds
/// every determinism ban, one entry per line, without the lines that
/// contain `omit`.
pub(crate) fn engine_clippy_toml(omit: Option<&str>) -> String {
    let mut lines = vec![
        "allow-unwrap-in-tests = true".to_owned(),
        "disallowed-methods = [".to_owned(),
        "    { path = \"std::panic::set_hook\", reason = \"use a returned error instead\" },"
            .to_owned(),
    ];
    lines.extend(crate::engine_clippy::DETERMINISM_BANS.iter().map(|path| {
        format!("    {{ path = \"{path}\", reason = \"use the deterministic form instead\" }},")
    }));
    lines.push("]".to_owned());
    lines.retain(|line| omit.is_none_or(|omit| !line.contains(omit)));
    lines.join("\n") + "\n"
}

/// Writes [`ROOT_CLIPPY_TOML`] at `root` and a passing Engine
/// `clippy.toml` at each Engine location the guard requires.
pub(crate) fn write_engine_clippy_tomls(root: &Path) {
    std::fs::write(root.join("clippy.toml"), ROOT_CLIPPY_TOML)
        .expect("the root clippy.toml writes");
    for dir in ["runvil-internal", "runvil", "runvil-plugin"] {
        let dir = root.join("crates").join(dir);
        std::fs::create_dir_all(&dir).expect("the Engine clippy.toml directory creates");
        std::fs::write(dir.join("clippy.toml"), engine_clippy_toml(None))
            .expect("the Engine clippy.toml writes");
    }
}
