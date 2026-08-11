//! Re-resolution: what one settled debounce window does, with no filesystem
//! watcher anywhere in it.
//!
//! [`Reloader::reload`] is the whole of a reload and takes no events, so it is
//! driven directly by a test and by the watcher alike. It re-reads
//! `prompts.toml`, runs the boot pass's own resolution over the prompts
//! directory under [`OnBroken::Retain`], and, when the text retrieval ranks on
//! moved, reindexes over the same loaded model. The new catalog and its index
//! are bound into one [`Generation`] and published by a single store into the
//! [`CatalogHandle`], so `need_prompt` and the runner can never disagree: a
//! reader loads the whole pair or the older whole pair, never a mix.
//!
//! Nothing is announced to a client. `tools/list` is the same four built-ins
//! whatever the catalog holds, and every call reads the catalog fresh, so a
//! prompt saved a moment ago is callable on the next call with no notification
//! and no reconnect.
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
use std::sync::atomic::AtomicBool;

use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::{Config, Secret};
use crate::error::{CatalogError, ConfigError};
use crate::generation::Generation;

/// What one reload changed.
///
/// A reload that could not resolve its candidate is a [`ReloadError`], not a
/// value of this type: every `Reload` describes a candidate that resolved. What
/// remains to report is whether the text retrieval ranks on moved, whether the
/// retrieval index that rode the swap is a stale carry-over, and whether this
/// build actually became the live generation or was dropped for a newer one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Reload {
    /// The text retrieval ranks on changed, so any index over it was rebuilt.
    pub(crate) ranking_changed: bool,
    /// The catalog resolved but its retrieval index could not be rebuilt, so
    /// the previous index rode the swap and is stale until the next reload.
    pub(crate) retrieval_stale: bool,
    /// This build became the live generation. `false` when a newer reload had
    /// already published, or a shutdown was in flight, so this build was dropped
    /// rather than allowed to clobber a fresher one.
    pub(crate) published: bool,
}

/// Why a reload did not resolve a candidate to publish.
///
/// Opaque and source-preserving: [`Watcher`](crate::Watcher) owns the logging
/// and tests classify with [`ReloadError::kind`]. It never reaches the crate's
/// public surface - the watcher is the boundary, and a reload failure keeps the
/// previous generation live rather than surfacing to a client - so the cause is
/// carried through [`std::error::Error::source`] without exposing a variant.
#[derive(Debug)]
pub(crate) struct ReloadError {
    repr: ReloadErrorRepr,
}

#[derive(Debug)]
enum ReloadErrorRepr {
    /// `prompts.toml` could not be read or parsed.
    Config(ConfigError),
    /// The candidate catalog could not be resolved.
    Catalog(CatalogError),
}

/// A stable classification of a [`ReloadError`], so a caller acts on the class
/// rather than matching the private representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the watcher acts on a reload failure through Display and source; the classifier exists for the tests and for a future caller that must branch on the class"
    )
)]
pub(crate) enum ReloadErrorKind {
    /// `prompts.toml` could not be read or parsed.
    Config,
    /// The candidate catalog could not be resolved.
    Catalog,
}

impl ReloadError {
    /// The configuration would not load.
    fn config(source: ConfigError) -> ReloadError {
        ReloadError {
            repr: ReloadErrorRepr::Config(source),
        }
    }

    /// The candidate catalog would not resolve.
    fn catalog(source: CatalogError) -> ReloadError {
        ReloadError {
            repr: ReloadErrorRepr::Catalog(source),
        }
    }

    /// Classifies the failure without exposing the error's representation.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "paired with ReloadErrorKind: the classifier the tests use and a future caller branches on"
        )
    )]
    pub(crate) fn kind(&self) -> ReloadErrorKind {
        match &self.repr {
            ReloadErrorRepr::Config(_) => ReloadErrorKind::Config,
            ReloadErrorRepr::Catalog(_) => ReloadErrorKind::Catalog,
        }
    }
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.repr {
            ReloadErrorRepr::Config(_) => {
                f.write_str("reload keeps the previous catalog: the configuration would not load")
            }
            ReloadErrorRepr::Catalog(_) => {
                f.write_str("reload keeps the previous catalog: the candidate would not resolve")
            }
        }
    }
}

impl std::error::Error for ReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            ReloadErrorRepr::Config(source) => Some(source),
            ReloadErrorRepr::Catalog(source) => Some(source),
        }
    }
}

/// The live state a reload replaces.
///
/// Cheap to share: everything it holds is either read-only or already behind an
/// `Arc`.
#[derive(Debug)]
pub(crate) struct Reloader {
    /// The `prompts.toml` the catalog-shaping tables are re-read from.
    source: PathBuf,
    /// The configuration boot loaded, which is what is actually in force.
    boot: Arc<Config>,
    /// The live generation a reload replaces, catalog and retrieval index
    /// together. It also owns the publication coordinator every reload claims a
    /// ticket from and publishes through, so ordering is global to the handle
    /// even when two reloaders share it.
    catalog: Arc<CatalogHandle>,
    /// Set by [`Watcher::shutdown`](crate::Watcher) so a reload that settles as
    /// the process stops does not publish a late generation.
    cancel: Arc<AtomicBool>,
}

/// A resolved reload that has claimed its place in line but has not published.
///
/// Splitting the resolve from the publish is what lets a test drive two reloads
/// past each other - build the older, build the newer, then commit them in the
/// reverse order - and prove the stale one is dropped rather than allowed to
/// win. Production calls [`Reloader::reload`], which builds then commits at once.
struct Pending {
    /// This reload's place in line, claimed when it began.
    ticket: u64,
    /// The catalog and its retrieval index, assembled and ready to publish.
    generation: Generation,
    /// What the commit will report, but for [`Reload::published`], which the
    /// commit fills in once it knows whether this build became live.
    outcome: Reload,
}

impl Reloader {
    /// Builds a reloader over the configuration file boot read and the live
    /// generation it produced.
    #[must_use]
    pub(crate) fn new(source: &Path, boot: Arc<Config>, catalog: Arc<CatalogHandle>) -> Reloader {
        Reloader {
            source: source.to_path_buf(),
            boot,
            catalog,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The shutdown flag the watcher shares, so signalling it stops both a
    /// pending reload's publish and the watch task.
    #[must_use]
    pub(crate) fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Re-resolves the catalog, swaps it in, and rebuilds retrieval when the
    /// text it ranks on moved, reporting what changed.
    ///
    /// This blocks: it reads and parses every prompt file, and a save that
    /// changed a name or a description also costs one embedding forward pass per
    /// prompt. It belongs on a blocking task, which is where the watcher calls
    /// it.
    ///
    /// # Errors
    /// Returns a [`ReloadError`] when the configuration will not load
    /// ([`ReloadErrorKind::Config`]) or the candidate catalog will not resolve
    /// ([`ReloadErrorKind::Catalog`]). The previous generation stays live in
    /// both cases; this runs under a live service, where the only useful answer
    /// to a candidate that cannot be resolved is to keep serving what already
    /// works and let the watcher log why.
    pub(crate) fn reload(&self) -> Result<Reload, ReloadError> {
        let pending = self.build()?;
        Ok(self.commit(pending))
    }

    /// Resolves a candidate generation and claims its ticket, without
    /// publishing.
    ///
    /// The ticket is claimed first, before any file is read, so it orders
    /// reloads by when they were triggered rather than by how long resolving
    /// took - which is what lets a slow reload be recognised as stale at commit.
    fn build(&self) -> Result<Pending, ReloadError> {
        let ticket = self.catalog.claim();
        let config = self.candidate_config()?;
        let candidate =
            Catalog::resolve(&config, OnBroken::Retain).map_err(ReloadError::catalog)?;

        let previous = self.catalog.load();
        let ranking_changed = previous.catalog().hash() != candidate.hash();
        let broken = candidate
            .entries()
            .iter()
            .filter(|entry| entry.problem().is_some())
            .count();
        // The whole generation - the new catalog and the index built over it -
        // is assembled here, off the runtime, before a single store publishes
        // it. A body-only save carries the previous index forward untouched; a
        // save that moved a name or a description reindexes over the same loaded
        // model, one forward pass per prompt and no weights. A rebuild that
        // fails carries the previous index forward too, now stale, rather than
        // dropping retrieval. Either way the pair is published atomically, so no
        // reader ever sees the new catalog beside the old index or the reverse.
        let (retrieval, retrieval_stale) = if ranking_changed {
            let reindex = previous.retrieval().rebuilt(&candidate);
            let stale = reindex.is_stale();
            (reindex.into_retrieval(), stale)
        } else {
            (previous.retrieval().clone(), false)
        };
        tracing::info!(
            "reloaded {} prompt(s), {broken} broken; ranking {}, retrieval {}",
            candidate.len(),
            if ranking_changed {
                "changed"
            } else {
                "unchanged"
            },
            if retrieval_stale { "stale" } else { "current" },
        );
        Ok(Pending {
            ticket,
            generation: Generation::new(candidate, retrieval),
            outcome: Reload {
                ranking_changed,
                retrieval_stale,
                published: false,
            },
        })
    }

    /// Publishes a [`Pending`] reload through the coordinator, filling in
    /// whether it became the live generation.
    fn commit(&self, pending: Pending) -> Reload {
        let Pending {
            ticket,
            generation,
            mut outcome,
        } = pending;
        outcome.published = self.catalog.publish(&self.cancel, ticket, generation);
        outcome
    }

    /// The configuration a reload resolves under: the file as it is now, with
    /// the settings a reload cannot change put back to what boot read.
    ///
    /// # Errors
    /// Returns a [`ReloadError`] with [`ReloadErrorKind::Config`] when the file
    /// could not be read or parsed.
    fn candidate_config(&self) -> Result<Config, ReloadError> {
        let mut config = Config::load(&self.source).map_err(ReloadError::config)?;
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
        Ok(config)
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
    if boot.server.token.as_ref().map(Secret::expose)
        != candidate.server.token.as_ref().map(Secret::expose)
    {
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
    if boot.server.allowed_hosts != candidate.server.allowed_hosts {
        ignored.push("[server].allowed_hosts");
    }
    if boot.paths.prompts != candidate.paths.prompts {
        ignored.push("[paths].prompts");
    }
    if boot.gateway.url != candidate.gateway.url {
        ignored.push("[gateway].url");
    }
    if boot.gateway.key.expose() != candidate.gateway.key.expose() {
        ignored.push("[gateway].key");
    }
    ignored
}
