//! Native asset resolution shared by ignored STT tests.

use std::path::{Path, PathBuf};

/// Resolves a required native fixture from an environment override or caller fallback root.
///
/// # Panics
///
/// Panics with the resolved path when the fixture is not a file.
#[must_use]
pub fn require_fixture(
    environment_variable: &str,
    fallback_root: &Path,
    fallback_name: &str,
) -> PathBuf {
    let path = std::env::var_os(environment_variable)
        .map_or_else(|| fallback_root.join(fallback_name), PathBuf::from);
    assert!(
        path.is_file(),
        "native test fixture is missing: {}",
        path.display()
    );
    path
}
