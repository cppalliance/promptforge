//! Re-resolution: what one settled debounce window does, with no filesystem
//! watcher anywhere in it.
//!
//! [`Reloader::reload`] is the whole of a reload and takes no events, so it is
//! driven directly by a test and by the watcher alike. It re-reads
//! `prompts.toml`, runs the boot pass's own resolution over the prompts
//! directory under [`OnBroken::Retain`], swaps the result into the
//! [`CatalogHandle`], rebuilds retrieval when the text it ranks on moved, and
//! announces the swap when the published tool set moved. The rebuild rides this
//! swap rather than a task of its own, so `need_prompt` and the runner never
//! disagree for longer than one atomic store.
//!
//! Two rules make a reload safe to run under a live service. A prompt that
//! fails validation becomes a broken entry carrying its error, so one typo
//! cannot freeze the rest of the catalog; and a fault about the catalog as a
//! whole - a stale override, two prompts under one name, an empty result, an
//! unparsable configuration - keeps the previous catalog and logs the reason,
//! because there is no partial answer to give.
//!
//! What does not reload is the service's own shape. `[server]` and `[gateway]`
//! were read once, at boot, by the transport and the run registry, and
//! `[paths].prompts` is the directory the watcher is watching. A change to any
//! of them is logged as ignored rather than half-applied.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;
use crate::retrieval::Retrieval;
use crate::watch::sessions::ListChanged;

/// What one reload changed.
///
/// The two flags are read by different things. `published_changed` is what sent
/// the `tools/list_changed` announcement, and covers every prompt's name,
/// description, exposure, and problem - the whole of what a client can read
/// about one; `ranking_changed` reports that
/// [`Catalog::hash`] moved, which is what tells retrieval its index is stale, so
/// an edit to a prompt's body alone costs no rebuild.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reload {
    /// The published tool set changed, and every session was told.
    pub published_changed: bool,
    /// The text retrieval ranks on changed, so any index over it is stale.
    pub ranking_changed: bool,
    /// The candidate was refused and the previous catalog is still live.
    pub refused: bool,
}

impl Reload {
    /// The reload that changed nothing because the candidate was refused.
    fn refusal() -> Reload {
        Reload {
            refused: true,
            ..Reload::default()
        }
    }
}

/// The live state a reload replaces, and where it announces the replacement.
///
/// Cheap to share: everything it holds is either read-only or already behind an
/// `Arc`.
#[derive(Debug)]
pub struct Reloader {
    /// The `prompts.toml` the catalog-shaping tables are re-read from.
    source: PathBuf,
    /// The configuration boot loaded, which is what is actually in force.
    boot: Arc<Config>,
    /// The catalog a reload swaps.
    catalog: Arc<CatalogHandle>,
    /// Where a changed tool set is announced.
    listener: Arc<dyn ListChanged>,
    /// The retrieval index a reload rebuilds when the ranking text moved.
    retrieval: Arc<Retrieval>,
}

impl Reloader {
    /// Builds a reloader over the configuration file boot read, the catalog it
    /// produced, and the retrieval index over that catalog.
    #[must_use]
    pub fn new(
        source: &Path,
        boot: Arc<Config>,
        catalog: Arc<CatalogHandle>,
        listener: Arc<dyn ListChanged>,
        retrieval: Arc<Retrieval>,
    ) -> Reloader {
        Reloader {
            source: source.to_path_buf(),
            boot,
            catalog,
            listener,
            retrieval,
        }
    }

    /// Re-resolves the catalog, swaps it in, and rebuilds retrieval when the
    /// text it ranks on moved, reporting what changed.
    ///
    /// This blocks: it reads and parses every prompt file, and a save that
    /// changed a name or a description also costs one embedding forward pass per
    /// prompt. It belongs on a blocking task, which is where the watcher calls
    /// it.
    ///
    /// A failure is not returned: this runs under a live service, where the only
    /// useful answer to a candidate that cannot be resolved is to keep serving
    /// the previous catalog and say so in the log. The refusal is visible in
    /// [`Reload::refused`].
    ///
    /// # Panics
    /// Panics if the announcement its listener makes needs a Tokio runtime and
    /// there is none. [`Sessions`](crate::watch::Sessions) is such a listener, so
    /// a reload over one must be called from inside a runtime - which the
    /// watcher's own `spawn_blocking` call satisfies.
    pub fn reload(&self) -> Reload {
        let Some(config) = self.candidate_config() else {
            return Reload::refusal();
        };
        let candidate = match Catalog::resolve(&config, OnBroken::Retain) {
            Ok(candidate) => candidate,
            Err(error) => {
                tracing::warn!("reload keeps the previous catalog: {error}");
                return Reload::refusal();
            }
        };

        let previous = self.catalog.load();
        let published_changed = published(&previous) != published(&candidate);
        let ranking_changed = previous.hash() != candidate.hash();
        let broken = candidate
            .entries()
            .iter()
            .filter(|entry| entry.problem().is_some())
            .count();
        tracing::info!(
            "reloaded {} prompt(s), {broken} broken; tools/list {}, ranking {}",
            candidate.len(),
            if published_changed {
                "changed"
            } else {
                "unchanged"
            },
            if ranking_changed {
                "changed"
            } else {
                "unchanged"
            },
        );
        // The swap is what a later call reads; a run already in flight holds the
        // snapshot it loaded and finishes under that definition.
        self.catalog.store(candidate);
        if ranking_changed {
            // After the swap, never before it: retrieval hands a name to a
            // caller that will pass it to the runner, so an index that is
            // briefly behind the catalog is safe in a way one that is briefly
            // ahead of it is not. The rebuild reuses the loaded model, so it
            // costs one forward pass per prompt and no weights - and it blocks
            // for the whole of that, which is why this method documents that it
            // belongs on a blocking task.
            self.retrieval.rebuild(&self.catalog.load());
        }
        if published_changed {
            self.listener.list_changed();
        }
        Reload {
            published_changed,
            ranking_changed,
            refused: false,
        }
    }

    /// The configuration a reload resolves under: the file as it is now, with
    /// the settings a reload cannot change put back to what boot read.
    ///
    /// `None` means the file could not be read or parsed, which is a refusal.
    fn candidate_config(&self) -> Option<Config> {
        let mut config = match Config::load(&self.source) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    "reload keeps the previous catalog: {} is not loadable: {error}",
                    self.source.display()
                );
                return None;
            }
        };
        let ignored = ignored_changes(&self.boot, &config);
        if !ignored.is_empty() {
            tracing::info!(
                "{} changed and does not reload; restart to apply it: {}",
                self.source.display(),
                ignored.join(", ")
            );
        }
        // The watcher watches the directory boot named, so resolving against a
        // different one would publish prompts nothing is watching.
        config.paths.prompts.clone_from(&self.boot.paths.prompts);
        Some(config)
    }
}

/// Which settings the file now names differently from the ones in force.
///
/// Every one of them was read once and wired into something a reload cannot
/// reach: the bound socket and the bearer layer, the run registry's limits, the
/// watcher's own window, the gateway each run goes through, and the directory
/// being watched. Naming them in the log is the difference between a setting
/// that does not reload and one that silently pretends to.
pub(super) fn ignored_changes(boot: &Config, candidate: &Config) -> Vec<&'static str> {
    let mut ignored = Vec::new();
    if boot.server.bind != candidate.server.bind {
        ignored.push("[server].bind");
    }
    if boot.server.token.expose() != candidate.server.token.expose() {
        ignored.push("[server].token");
    }
    if boot.server.max_concurrent_runs != candidate.server.max_concurrent_runs {
        ignored.push("[server].max_concurrent_runs");
    }
    if boot.server.admission_timeout != candidate.server.admission_timeout {
        ignored.push("[server].admission_timeout");
    }
    if boot.server.reply_deadline != candidate.server.reply_deadline {
        ignored.push("[server].reply_deadline");
    }
    if boot.server.retain_completed != candidate.server.retain_completed {
        ignored.push("[server].retain_completed");
    }
    if boot.server.watch != candidate.server.watch {
        ignored.push("[server].watch");
    }
    if boot.server.watch_debounce != candidate.server.watch_debounce {
        ignored.push("[server].watch_debounce");
    }
    if boot.paths.prompts != candidate.paths.prompts {
        ignored.push("[paths].prompts");
    }
    if boot.gateway.url != candidate.gateway.url {
        ignored.push("[gateway].url");
    }
    if boot.gateway.token.expose() != candidate.gateway.token.expose() {
        ignored.push("[gateway].token");
    }
    if boot.gateway.model != candidate.gateway.model {
        ignored.push("[gateway].model");
    }
    ignored
}

/// The published surface as one line per prompt: its name, its description, its
/// exposure, and its problem.
///
/// Those four are exactly what a client can read about a prompt, whether it
/// reads them from `tools/list` or from `list_prompts`, so comparing them is
/// what makes the announcement mean what it says. Comparing the serialized tool
/// list instead would be narrower than the contract: a listed prompt's
/// description is part of what a caller sees and none of what `tools/list`
/// carries, so a renamed listed prompt would change the catalog visibly and
/// announce nothing.
///
/// A separator that cannot occur in any of the four keeps two fields from
/// running together into one that happens to match.
fn published(catalog: &Catalog) -> Vec<String> {
    catalog
        .entries()
        .iter()
        .map(|entry| {
            format!(
                "{}\u{1f}{}\u{1f}{:?}\u{1f}{}",
                entry.name(),
                entry.description(),
                entry.expose(),
                entry.problem().unwrap_or_default()
            )
        })
        .collect()
}
