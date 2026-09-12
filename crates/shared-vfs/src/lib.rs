//! Generic virtual filesystem machinery: canonical interned paths, the
//! claims model, the mount router, and backends.
//!
//! This crate is the permanent bottom of the dependency stack: std only,
//! no workspace or external crates, and no promptforge policy (no
//! `/_promptforge` paths, no Store, no run concepts).

mod error;
mod glob;
mod handle;
mod host;
mod memory;
mod path;
mod router;
mod traits;
mod types;

pub use error::VfsError;
pub use handle::{Access, VfsRef};
pub use host::HostBackend;
pub use memory::MemoryBackend;
pub use path::{VfsPath, VfsPathBuf};
pub use router::VfsRefBuilder;
pub use traits::{AllowAll, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};
pub use types::{Entry, FileType, GrepMatch, GrepQuery, GrepResults, Stat};

#[cfg(test)]
mod tests {
    /// A manifest section is a dependency table when it is exactly one of
    /// the three dependency tables, a sub-table of one
    /// (`[dependencies.foo]` declares a dependency the same way), or a
    /// target-qualified dependency table.
    fn is_dependency_table(section: &str) -> bool {
        const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
        TABLES.iter().any(|table| {
            section == *table
                || section
                    .strip_prefix(*table)
                    .is_some_and(|rest| rest.starts_with('.'))
        }) || (section.starts_with("target.") && section.ends_with(".dependencies"))
    }

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
            assert!(
                !is_dependency_table(&section),
                "zero-dependency rule violated: [{section}] declares `{line}`"
            );
        }
        Ok(())
    }

    #[test]
    fn dependency_sub_tables_count_as_dependency_tables() {
        // Regression: `[dependencies.foo]` once slipped past the exact-
        // match section check while still declaring a dependency.
        for section in [
            "dependencies",
            "dependencies.foo",
            "dev-dependencies",
            "dev-dependencies.foo",
            "build-dependencies",
            "build-dependencies.foo",
            "target.'cfg(windows)'.dependencies",
        ] {
            assert!(is_dependency_table(section), "[{section}] must be caught");
        }
        for section in ["package", "lints", "features", "dependenciesfoo"] {
            assert!(!is_dependency_table(section), "[{section}] must pass");
        }
    }
}
