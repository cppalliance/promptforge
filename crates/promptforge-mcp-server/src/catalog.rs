//! The resolved catalog: which prompts the harness may reach, and the pass that
//! works them out from `prompts.toml` and the prompts directory.
//!
//! Resolution is one function. Boot and the watcher's reload run the same globs,
//! the same named-block exceptions, and the same per-prompt checks; they differ
//! only in what a prompt that fails validation costs, which is the [`OnBroken`]
//! parameter. Boot refuses to start with an incomplete catalog, because a client
//! discovers a silently missing prompt as a missing tool with no explanation. A
//! reload keeps the broken prompt as a [broken entry](Entry::problem) carrying
//! its error, because one typo in one file must not freeze every other prompt in
//! the catalog.
//!
//! The live catalog sits in a [`CatalogHandle`], swapped whole. A run in flight
//! holds the `Arc<Catalog>` it loaded and finishes under the definition it
//! started with.

#[cfg(test)]
mod fixture;
mod resolve;
#[cfg(test)]
mod tests;

use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use promptforge_core::parser::Prompt;

use crate::config::Config;
use crate::error::CatalogError;

/// What a resolution pass does with a prompt that fails validation.
///
/// This is the only difference between the boot pass and the watcher's reload,
/// and it is the consequence rather than the checks: both run every check in
/// exactly the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnBroken {
    /// A failing prompt is a fault, and the pass returns an error naming it
    /// alongside every other fault. Boot's rule.
    Reject,
    /// A failing prompt becomes an entry whose [`Entry::problem`] carries the
    /// error: still listed, and answering a call with the failure rather than
    /// running a stale copy. The reload's rule.
    Retain,
}

/// One prompt the harness may reach.
///
/// An entry is either healthy - [`prompt`](Self::prompt) is `Some` and
/// [`problem`](Self::problem) is `None` - or broken, which is the reverse. A
/// broken entry keeps its place in the catalog so the failure is visible where
/// the prompt used to be.
#[derive(Debug, Clone)]
pub struct Entry {
    name: String,
    description: String,
    version: u32,
    path: PathBuf,
    source: Option<String>,
    prompt: Option<Prompt>,
    problem: Option<String>,
}

impl Entry {
    /// Builds a healthy entry from a parsed prompt.
    pub(crate) fn healthy(path: PathBuf, source: String, prompt: Prompt) -> Entry {
        Entry {
            name: prompt.frontmatter.name.clone(),
            description: prompt.frontmatter.description.clone(),
            version: prompt.frontmatter.version,
            path,
            source: Some(source),
            prompt: Some(prompt),
            problem: None,
        }
    }

    /// Builds a broken entry under the best name the pass could work out: the
    /// frontmatter name where the file parsed, the `[prompts.NAME]` key where a
    /// named block reached it, and the file stem otherwise.
    pub(crate) fn broken(name: String, path: PathBuf, problem: impl Into<String>) -> Entry {
        Entry {
            name,
            description: String::new(),
            version: 0,
            path,
            source: None,
            prompt: None,
            problem: Some(problem.into()),
        }
    }

    /// The prompt's frontmatter name, which is how a caller names it to
    /// `run_prompt`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The prompt's frontmatter description, empty on a broken entry.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The prompt's frontmatter contract version, zero on a broken entry.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The file the entry was resolved from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The validated source snapshot, or `None` on a broken entry.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// The parsed prompt, or `None` on a broken entry.
    #[must_use]
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The validation error, or `None` on a healthy entry.
    #[must_use]
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }
}

/// Every prompt one resolution pass produced, ordered by name.
///
/// The order is a function of the catalog's contents alone, so two passes over
/// an unchanged directory produce the same catalog whatever order the
/// filesystem enumerated it in.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<Entry>,
}

impl Catalog {
    /// Resolves the catalog from a configuration and the prompts directory.
    ///
    /// Expands `[catalog].include`, subtracts `[catalog].exclude`, then applies
    /// the `[prompts.NAME]` blocks: one may promote a globbed prompt, drop it,
    /// or reach a file no glob matches. A glob-matched file that declares no
    /// `promptforge:` frontmatter version is not a prompt and is skipped without
    /// comment; every other resolved file must be readable, must parse, and must
    /// yield a name matching `^[a-z][a-z0-9_]{0,47}$`.
    ///
    /// # Errors
    /// Returns [`CatalogError`] carrying every fault the pass found: an invalid
    /// glob pattern, a `[prompts.NAME]` block with no `file` that matches no
    /// globbed prompt, two prompts declaring one name, an empty resolved
    /// catalog, and - under [`OnBroken::Reject`] - each prompt that failed
    /// validation. The pass always runs to completion first.
    pub fn resolve(config: &Config, on_broken: OnBroken) -> Result<Catalog, CatalogError> {
        resolve::resolve(config, on_broken)
    }

    /// Builds a catalog from entries, ordering them by name.
    pub(crate) fn new(mut entries: Vec<Entry>) -> Catalog {
        entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        Catalog { entries }
    }

    /// Every entry, ordered by name.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many prompts the catalog holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog holds no prompts.
    ///
    /// A resolution pass never produces one: an empty result is a fault under
    /// both [`OnBroken`] values. The method is here for a caller holding a
    /// catalog built by other means.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry whose name is exactly `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// A content hash over every entry's name and description.
    ///
    /// This is what tells a reload whether a save changed anything retrieval
    /// ranks on, so an edit to a prompt's body alone skips the rebuild. It is a
    /// value to compare against another hash from the same process, not one to
    /// persist: the standard library's default hasher is deterministic within a
    /// build and free to change between them.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for entry in &self.entries {
            entry.name.hash(&mut hasher);
            entry.description.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The live catalog, swapped whole.
///
/// Readers load an `Arc` snapshot and keep it for as long as they need it, so a
/// run in flight across a reload finishes under the catalog it started with.
#[derive(Debug)]
pub struct CatalogHandle {
    current: ArcSwap<Catalog>,
}

impl CatalogHandle {
    /// Holds a catalog, ready to be read and later replaced.
    #[must_use]
    pub fn new(catalog: Catalog) -> CatalogHandle {
        CatalogHandle {
            current: ArcSwap::from_pointee(catalog),
        }
    }

    /// A snapshot of the catalog as it is now.
    #[must_use]
    pub fn load(&self) -> Arc<Catalog> {
        self.current.load_full()
    }

    /// Replaces the catalog. Snapshots already handed out are unaffected.
    pub fn store(&self, catalog: Catalog) {
        self.current.store(Arc::new(catalog));
    }
}
