//! Generic virtual filesystem machinery: canonical interned paths, the
//! claims model, the mount router, and backends.
//!
//! This crate is the permanent bottom of the dependency stack: std only,
//! no workspace or external crates, and no promptforge policy (no
//! `/_promptforge` paths, no Store, no run concepts).

mod error;
mod path;
mod traits;
mod types;

pub use error::VfsError;
pub use path::{VfsPath, VfsPathBuf};
pub use traits::{AllowAll, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};
pub use types::{Entry, FileType, GrepMatch, GrepQuery, GrepResults, Stat};

#[cfg(test)]
mod tests {
    /// The zero-dependency rule is load-bearing: this crate compiles alone
    /// and never rebuilds for a dependency rev, so the manifest must never
    /// declare a dependency. This test reads the crate's own Cargo.toml and
    /// fails if any dependency table carries an entry.
    #[test]
    fn the_manifest_declares_no_dependencies() -> Result<(), std::io::Error> {
        let manifest = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        )?;
        let mut section = String::new();
        for raw_line in manifest.lines() {
            let line = raw_line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_owned();
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let is_dependency_table = section == "dependencies"
                || section == "dev-dependencies"
                || section == "build-dependencies"
                || (section.starts_with("target.") && section.ends_with(".dependencies"));
            assert!(
                !is_dependency_table,
                "zero-dependency rule violated: [{section}] declares `{line}`"
            );
        }
        Ok(())
    }
}
