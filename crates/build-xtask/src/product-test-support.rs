//! Shared fixtures for the product-boundary test siblings.

use std::path::{Path, PathBuf};

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// Writes a minimal crate manifest into a fake workspace; `dir_name` may
/// contain a slash to nest the crate under a container (`promptforge/lua`).
pub(crate) fn write_crate(root: &Path, dir_name: &str, package: &str, deps: &str) {
    let dir = root.join("crates").join(dir_name);
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\n{deps}"),
    )
    .expect("the manifest writes");
}
