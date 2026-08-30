//! Provenance recorded during profile merging: which file each part of the
//! merged document came from.
//!
//! The merge pass (`merge.rs`) records origins here as a side channel, so the
//! recording never changes the merged value. [`Config`](crate::Config) carries
//! the result and renders it through `Config::to_json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The origin file of every keyed-array entry and every written path in a
/// merged profile document.
///
/// Keyed-array entries (`[[model]]` and `[[local_model]]` by `name`,
/// `[[endpoint]]` and `[[dominion]]` by `id`) map to the file whose
/// definition won the merge. Scalar and table writes map by dotted TOML path
/// (`server`, `local.cache_dir`, `tools.web_search`) to the file that last
/// wrote them; a wholesale-inserted table records every path beneath it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Provenance {
    /// Keyed-array entry origins: `(array name, identity)` to source file.
    entries: HashMap<(String, String), PathBuf>,
    /// Dotted-path origins for scalar and table writes.
    paths: HashMap<String, PathBuf>,
}

impl Provenance {
    /// Record that the keyed-array entry `identity` in `array_name` was
    /// written from `source`.
    pub(crate) fn record_entry(&mut self, array_name: &str, identity: &str, source: &Path) {
        self.entries.insert(
            (array_name.to_owned(), identity.to_owned()),
            source.to_path_buf(),
        );
    }

    /// Record that the dotted `path` was written from `source`.
    pub(crate) fn record_path(&mut self, path: &str, source: &Path) {
        self.paths.insert(path.to_owned(), source.to_path_buf());
    }

    /// The file a keyed-array entry's winning definition came from.
    pub(crate) fn entry_source(&self, array_name: &str, identity: &str) -> Option<&Path> {
        self.entries
            .get(&(array_name.to_owned(), identity.to_owned()))
            .map(PathBuf::as_path)
    }

    /// Iterate the recorded dotted paths and their source files.
    pub(crate) fn paths(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.paths
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_path()))
    }
}
