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
// The unit tests for this module would normally live inline in a
// `#[cfg(test)] mod tests` block here. Inlining them would push this file past
// the 500-line ceiling, and that gate wins, so they stay in a sibling child
// module beside the code they cover rather than in a separate file elsewhere.
#[cfg(test)]
mod tests;

use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use promptforge_core::parser::Prompt;

use crate::config::Config;
use crate::error::{CatalogError, FaultKind};
use crate::generation::Generation;
use crate::retrieval::Retrieval;

/// What a resolution pass does with a prompt that fails validation.
///
/// This is the only difference between the boot pass and the watcher's reload,
/// and it is the consequence rather than the checks: both run every check in
/// exactly the same order.
///
/// # Examples
/// ```no_run
/// use promptforge_mcp_server::{Catalog, Config, OnBroken};
///
/// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
/// // Boot rejects any fault outright; a reload retains a broken prompt as a
/// // listed entry so one typo cannot freeze the rest of the catalog.
/// let boot = Catalog::resolve(config, OnBroken::Reject)?;
/// let reload = Catalog::resolve(config, OnBroken::Retain)?;
/// # let _ = (boot, reload);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnBroken {
    /// A failing prompt is a fault, and the pass returns an error naming it
    /// alongside every other fault. Boot's rule.
    Reject,
    /// A failing prompt becomes a broken entry that carries the error: still
    /// listed, and answering a call with the failure rather than running a stale
    /// copy. The reload's rule.
    Retain,
}

/// One prompt the harness may reach.
///
/// The loaded part of an entry is a resolved prompt XOR a problem, held in a
/// private [`EntryState`] so the illegal fourth combination - both, or neither -
/// cannot be built by hand. An entry is healthy when the state is
/// [`EntryState::Healthy`], carrying the validated source and the parsed prompt;
/// it is broken otherwise, carrying the fault that stops it. A broken entry
/// keeps its place in the catalog so the failure is visible where the prompt
/// used to be.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    name: String,
    description: String,
    path: PathBuf,
    state: EntryState,
}

/// The XOR at the heart of an [`Entry`]: a healthy prompt or the problem that
/// stops it, never both and never neither.
#[derive(Debug, Clone)]
enum EntryState {
    /// The file read, parsed, and passed every check.
    Healthy {
        source: String,
        prompt: Box<Prompt>,
    },
    /// The file failed a check. The class is what a fault is tagged with; the
    /// detail is the human-readable line.
    Broken { kind: FaultKind, detail: String },
}

impl Entry {
    /// Builds a healthy entry from a parsed prompt.
    pub(crate) fn healthy(path: PathBuf, source: String, prompt: Prompt) -> Entry {
        Entry {
            name: prompt.frontmatter().name().to_owned(),
            description: prompt.frontmatter().description().to_owned(),
            path,
            state: EntryState::Healthy {
                source,
                prompt: Box::new(prompt),
            },
        }
    }

    /// Builds a broken entry whose fault class is inferred as an unparsable
    /// file. The resolver uses [`Entry::broken_as`] to tag the real class.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn broken(name: String, path: PathBuf, problem: impl Into<String>) -> Entry {
        Entry::broken_as(name, path, FaultKind::Unparsable, problem)
    }

    /// Builds a broken entry under the best name the pass could work out - the
    /// frontmatter name where the file parsed, the `[prompts.NAME]` key where a
    /// named block reached it, the file stem otherwise - tagged with the class
    /// of fault that stops it.
    pub(crate) fn broken_as(
        name: String,
        path: PathBuf,
        kind: FaultKind,
        problem: impl Into<String>,
    ) -> Entry {
        Entry {
            name,
            description: String::new(),
            path,
            state: EntryState::Broken {
                kind,
                detail: problem.into(),
            },
        }
    }

    /// The prompt's frontmatter name, which is how a caller names it to
    /// `run_prompt`.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The prompt's frontmatter description, empty on a broken entry.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// The file the entry was resolved from.
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The validated source snapshot, or `None` on a broken entry.
    #[must_use]
    pub(crate) fn source(&self) -> Option<&str> {
        match &self.state {
            EntryState::Healthy { source, .. } => Some(source),
            EntryState::Broken { .. } => None,
        }
    }

    /// The parsed prompt, or `None` on a broken entry.
    #[must_use]
    pub(crate) fn prompt(&self) -> Option<&Prompt> {
        match &self.state {
            EntryState::Healthy { prompt, .. } => Some(prompt.as_ref()),
            EntryState::Broken { .. } => None,
        }
    }

    /// The validation error, or `None` on a healthy entry.
    #[must_use]
    pub(crate) fn problem(&self) -> Option<&str> {
        match &self.state {
            EntryState::Broken { detail, .. } => Some(detail),
            EntryState::Healthy { .. } => None,
        }
    }

    /// The class of the fault on a broken entry, or `None` on a healthy one, so
    /// resolution can raise the fault under the class the admission step already
    /// knew rather than re-deriving it from the detail string.
    #[must_use]
    pub(crate) fn problem_kind(&self) -> Option<FaultKind> {
        match &self.state {
            EntryState::Broken { kind, .. } => Some(*kind),
            EntryState::Healthy { .. } => None,
        }
    }
}

/// Every prompt one resolution pass produced, ordered by name.
///
/// The order is a function of the catalog's contents alone, so two passes over
/// an unchanged directory produce the same catalog whatever order the
/// filesystem enumerated it in.
///
/// # Examples
/// ```no_run
/// use promptforge_mcp_server::{Catalog, Config, OnBroken};
///
/// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
/// // Boot resolves the catalog and refuses to start on any fault.
/// let catalog = Catalog::resolve(config, OnBroken::Reject)?;
/// # let _ = catalog;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Catalog {
    entries: Vec<Entry>,
    /// Name to index, for O(1) exact-name lookup that keeps the sorted slice
    /// intact. Only tests resolve a name this way today, so the map is built
    /// only under test; production reaches an entry by iterating [`entries`].
    #[cfg(test)]
    by_name: std::collections::HashMap<String, usize>,
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
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_mcp_server::{Catalog, Config, OnBroken};
    ///
    /// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    /// // A reload keeps a broken prompt as a listed entry rather than failing.
    /// let catalog = Catalog::resolve(config, OnBroken::Retain)?;
    /// # let _ = catalog;
    /// # Ok(())
    /// # }
    /// ```
    pub fn resolve(config: &Config, on_broken: OnBroken) -> Result<Catalog, CatalogError> {
        resolve::resolve(config, on_broken)
    }

    /// Builds a catalog from entries, ordering them by name, then a healthy
    /// entry ahead of a broken one, then by path.
    ///
    /// The healthy-before-broken tie-break is what stops a broken entry whose
    /// placeholder name (a file stem, say `drafts/research_person.md`) happens
    /// to equal a healthy prompt's declared name from shadowing it: a lookup
    /// that takes the first match by name now reaches the healthy prompt, and
    /// the broken placeholder never resolves as if it were valid.
    pub(crate) fn new(mut entries: Vec<Entry>) -> Catalog {
        entries.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.problem().is_some().cmp(&b.problem().is_some()))
                .then_with(|| a.path.cmp(&b.path))
        });
        // The first entry for a name wins the index, and healthy sorts before
        // broken, so a lookup resolves to the healthy prompt whenever a broken
        // placeholder shares its name.
        #[cfg(test)]
        let by_name = {
            let mut map = std::collections::HashMap::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                map.entry(entry.name.clone()).or_insert(index);
            }
            map
        };
        Catalog {
            entries,
            #[cfg(test)]
            by_name,
        }
    }

    /// Every entry, ordered by name.
    #[must_use]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many prompts the catalog holds.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog holds no prompts.
    ///
    /// A resolution pass never produces one: an empty result is a fault under
    /// both [`OnBroken`] values. The method is here for a caller holding a
    /// catalog built by other means.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry whose name is exactly `name`, resolved through the name index
    /// in O(1). A healthy prompt is preferred over a broken placeholder that
    /// shares its name, so a broken entry never shadows the prompt it sits next
    /// to.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn find(&self, name: &str) -> Option<&Entry> {
        self.by_name.get(name).map(|&index| &self.entries[index])
    }

    /// A content hash over every entry's name and description.
    ///
    /// This is what tells a reload whether a save changed anything retrieval
    /// ranks on, so an edit to a prompt's body alone skips the rebuild. It is a
    /// value to compare against another hash from the same process, not one to
    /// persist: the standard library's default hasher is deterministic within a
    /// build and free to change between them.
    #[must_use]
    pub(crate) fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for entry in &self.entries {
            entry.name.hash(&mut hasher);
            entry.description.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The live generation - a catalog and the retrieval index over it - swapped
/// whole.
///
/// Readers load an `Arc` snapshot and keep it for as long as they need it, so a
/// run in flight across a reload finishes under the generation it started with,
/// and the catalog it resolves names in always agrees with the index
/// `need_prompt` ranks against, because the two are one value behind one swap.
///
/// # Examples
/// ```no_run
/// use promptforge_mcp_server::{Catalog, CatalogHandle, Config, OnBroken};
///
/// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
/// let catalog = Catalog::resolve(config, OnBroken::Reject)?;
/// // The live catalog sits behind a handle, swapped whole on reload.
/// let handle = CatalogHandle::new(catalog);
/// # let _ = handle;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct CatalogHandle {
    current: ArcSwap<Generation>,
    /// The single publication coordinator every reload over this handle claims a
    /// ticket from and publishes through, so publication order is global to the
    /// handle. Two reloaders (two [`Watcher::start`](crate::Watcher::start) calls)
    /// sharing one handle cannot publish stale-over-fresh: a slower reload on
    /// either loses the compare and is dropped.
    coordinator: Publisher,
}

/// The publication coordinator owned by a [`CatalogHandle`].
///
/// Each reload claims a monotonically increasing [ticket](CatalogHandle::claim)
/// at the moment it begins, so the newest trigger always carries the highest
/// number across every reloader over the handle. Publishing then takes the store
/// lock and compares: a ticket no newer than the last one published is a stale
/// build - a reload that started earlier but finished later - and is dropped
/// rather than stored. The lock spans the whole compare-and-store, so two
/// reloads cannot interleave their publishes and the last-writer-wins race the
/// single `ArcSwap` left open is closed: order is decided by trigger time, not by
/// which build happens to finish first, and not by which reloader ran it.
#[derive(Debug, Default)]
struct Publisher {
    /// Assigns each reload its ticket. Relaxed is enough: the ordering that
    /// matters is imposed by the store lock, not by this counter.
    tickets: AtomicU64,
    /// The highest ticket whose generation has been published, guarded so the
    /// compare-and-store is atomic against another reload's publish.
    published: Mutex<u64>,
}

impl CatalogHandle {
    /// Holds a catalog with no retrieval index, ready to be read and later
    /// replaced. A `need_prompt` call against it reports that retrieval is
    /// unavailable until a generation carrying an index is stored.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_mcp_server::{Catalog, CatalogHandle, Config, OnBroken};
    ///
    /// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    /// let catalog = Catalog::resolve(config, OnBroken::Reject)?;
    /// let handle = CatalogHandle::new(catalog);
    /// # let _ = handle;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn new(catalog: Catalog) -> CatalogHandle {
        CatalogHandle::with_retrieval(catalog, Retrieval::idle())
    }

    /// Holds a catalog and the retrieval index built over it as one generation.
    #[must_use]
    pub(crate) fn with_retrieval(catalog: Catalog, retrieval: Retrieval) -> CatalogHandle {
        CatalogHandle {
            current: ArcSwap::from_pointee(Generation::new(catalog, retrieval)),
            coordinator: Publisher::default(),
        }
    }

    /// A snapshot of the live generation as it is now.
    #[must_use]
    pub(crate) fn load(&self) -> Arc<Generation> {
        self.current.load_full()
    }

    /// Replaces the live generation. Snapshots already handed out are
    /// unaffected.
    pub(crate) fn store(&self, generation: Generation) {
        self.current.store(Arc::new(generation));
    }

    /// Claims the next publication ticket for a reload about to begin.
    ///
    /// The first ticket is `1`, so it is always newer than the `0` no publish
    /// has beaten yet. Every reloader over this handle claims from the same
    /// counter, so the order is global to the handle: the newest trigger carries
    /// the highest number regardless of which reloader ran it.
    pub(crate) fn claim(&self) -> u64 {
        self.coordinator
            .tickets
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    /// Publishes `generation` under `ticket` unless a newer generation has
    /// already been published through this handle or `cancel` is set, and reports
    /// whether it became live. The compare-and-store is serialized on the
    /// handle's own lock, so two reloaders committing out of order over one shared
    /// handle cannot publish a stale generation over a fresher one: the older
    /// ticket loses the compare and is dropped, leaving the previous generation
    /// exactly as it was.
    pub(crate) fn publish(&self, cancel: &AtomicBool, ticket: u64, generation: Generation) -> bool {
        let mut published = self
            .coordinator
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cancel.load(Ordering::SeqCst) {
            // A shutdown is in flight: a late generation must not be published
            // over a catalog the process is about to stop serving.
            return false;
        }
        if ticket <= *published {
            // A newer reload already won; this older build would be a lost
            // update, so it is dropped.
            return false;
        }
        self.store(generation);
        *published = ticket;
        true
    }
}
