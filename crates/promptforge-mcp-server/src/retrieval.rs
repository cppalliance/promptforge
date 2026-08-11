//! `need_prompt`: the prompts closest to a capability, ranked but never chosen.
//!
//! A caller states a capability and gets up to three prompts back, best first,
//! and chooses among them itself with the whole conversation in front of it. It
//! then calls `run_prompt` with a name it was handed rather than one it guessed.
//! Nothing here runs a prompt: a retriever that picked wrong would spend minutes
//! of gateway time producing an artifact nobody asked for, and the caller could
//! not tell that it had.
//!
//! Three rules shape what is offered.
//!
//! The similarity floor is set as low as the engine's validated policy allows,
//! which is zero. The engine's own default was tuned for author-register prose
//! and abstains on a conversationally phrased need, and a floor exists to stop
//! an unattended binding - which is not what happens here, because a model reads
//! the candidates and decides. Three weak candidates are self-evidently weak to
//! that reader, while an empty answer to a casually phrased request helps
//! nobody. The engine forbids a negative floor, so the one case still withheld
//! is a candidate whose similarity to the capability is negative; a zero floor
//! is the closest this module can come to offering everything and is accurate to
//! what the engine will admit.
//!
//! A broken prompt is never a candidate. It cannot run, so offering it would
//! spend the caller's next call on a certain failure, and a broken entry carries
//! no description, so there is nothing to rank it on either. `list_prompts` is
//! where a broken prompt and its problem are read.
//!
//! Retrieval is optional twice over, and neither absence stops the server. The
//! `picker` feature compiles the ranking engine and its embedded weights in, and
//! a build without it publishes no `need_prompt` at all. With the feature, the
//! model is loaded once at boot; a load that fails is logged and the process
//! serves on, because every prompt is still callable and refusing to start over
//! a tool that only shortens a search would be the worse trade. `need_prompt`
//! then answers that retrieval is unavailable, which is information the calling
//! model can act on.

// Two `cfg` attributes rather than one `all(..)`, because the lint policy reads
// a bare `cfg(test)` to know that a fixture may `expect` on its own setup.
#[cfg(test)]
#[cfg(feature = "picker")]
pub(crate) mod fixture;
#[cfg(feature = "picker")]
pub(crate) mod index;
#[cfg(test)]
#[cfg(feature = "picker")]
mod tests;

#[cfg(feature = "picker")]
use std::sync::Arc;

#[cfg(feature = "picker")]
use serde::Serialize;

use crate::catalog::Catalog;

/// How many prompts one shortlist offers at most.
///
/// Three, because the engine's own measurements put the right tool in the top
/// three far more often than at the top, and a caller that reads three lines
/// loses nothing by the other two being wrong.
#[cfg(feature = "picker")]
const CANDIDATES: usize = 3;

/// One prompt offered for a capability.
///
/// The name is what `run_prompt` takes, which is the whole point of handing it
/// over: the caller no longer has to guess one.
///
/// Only a build with the `picker` feature ever produces a candidate: without
/// it there is no ranking engine, so `need_prompt` is not published and this
/// type has no constructor.
#[cfg(feature = "picker")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Candidate {
    /// The prompt's name, as `run_prompt` and `list_prompts` spell it.
    pub(crate) name: String,
    /// The prompt's description, verbatim from its frontmatter.
    pub(crate) description: String,
}

/// What one retrieval attempt produced.
///
/// The three arms are three different situations for the caller, which is why
/// they are not collapsed into a `Result<Vec<_>>`: an empty
/// [`Candidates`](Shortlist::Candidates) is a complete answer, and the other two
/// say that no answer was reached and which side the reason sits on.
///
/// Only a `picker` build ranks anything, so the whole type is gated on that
/// feature: without it there is no shortlist to describe.
#[cfg(feature = "picker")]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum Shortlist {
    /// The prompts closest to the capability, best first, at most three of
    /// them. Empty when the catalog offered none.
    Candidates(Vec<Candidate>),
    /// Retrieval is not part of this build, or its model could not be loaded.
    Unavailable,
    /// The engine could not embed the capability, carrying the reason.
    Failed(String),
}

/// One immutable retrieval index: the engine over one catalog's descriptors.
///
/// A `Retrieval` is a value, not a cell. It is built once - at boot, or off the
/// previous one when a save changes what it ranks on - and never mutated after.
/// The live one lives inside a generation beside the catalog it was built over,
/// and a reload publishes a fresh generation carrying a fresh `Retrieval`, so
/// the pair a reader loads is always consistent: the index it holds ranks
/// exactly the catalog it holds.
///
/// Cloning is cheap - the built index sits behind an `Arc` - which is what lets
/// a body-only save carry the previous index into the next generation untouched.
#[derive(Clone)]
#[non_exhaustive]
pub(crate) struct Retrieval {
    /// The engine over the catalog's descriptors, absent when no index was
    /// built - a build without the `picker` feature, or a model that would not
    /// load.
    #[cfg(feature = "picker")]
    index: Option<Arc<index::Index>>,
}

impl Retrieval {
    /// Retrieval that answers nothing, for a server built without an index.
    ///
    /// Every `need_prompt` call against it reports
    /// [`Shortlist::Unavailable`]. This is what a build without the `picker`
    /// feature has, and what a test that is not about retrieval uses.
    #[must_use]
    pub(crate) fn idle() -> Retrieval {
        Retrieval {
            #[cfg(feature = "picker")]
            index: None,
        }
    }

    /// Builds the index over `catalog`, loading the embedding model once.
    ///
    /// This is the slow call in the server's boot: it parses tens of megabytes
    /// of compiled-in weights and embeds every runnable prompt. It cannot fail.
    /// A model that will not load is reported at error level and the result is
    /// an idle index, because the rest of the server - every
    /// prompt, both listing tools, the runner - is unaffected, and a harness
    /// that cannot start its MCP server is worse off than one whose retrieval
    /// tool says it is unavailable.
    #[must_use]
    pub(crate) fn start(catalog: &Catalog) -> Retrieval {
        Retrieval::built(catalog)
    }

    /// The prompts closest to `capability`, best first.
    ///
    /// The capability is embedded by the model the prompts were embedded with
    /// and every prompt is scored against it; no candidate is withheld for
    /// scoring weakly but positively, since the caller is a model that can see
    /// for itself how weak three candidates are. Only a negative similarity is
    /// withheld, which the engine's validated floor domain forces.
    ///
    /// This blocks for as long as embedding the capability takes - one forward
    /// pass through the model - so it belongs on a blocking task, which is where
    /// the handler that answers `need_prompt` calls it. The index is immutable,
    /// so a rebuild that lands mid-call is a different generation entirely and
    /// cannot show a caller a half-built index.
    ///
    /// Only a `picker` build ranks, so this exists only there.
    #[cfg(feature = "picker")]
    #[must_use]
    pub(crate) fn shortlist(&self, capability: &str) -> Shortlist {
        self.ranked(capability)
    }

    /// A fresh retrieval over `catalog`, reusing this one's loaded model,
    /// reported as a typed [`Reindex`] outcome.
    ///
    /// The answer to a save that changed a name or a description. It costs one
    /// forward pass per prompt and no weights, which is what lets it ride the
    /// same generation swap the watcher already performs. An idle retrieval
    /// stays idle: there is no model behind it to reuse, and loading one here
    /// would put the boot's cost on a filesystem event.
    ///
    /// This blocks for as long as the embedding takes, so it belongs on a
    /// blocking task - which is where the reload that calls it already runs.
    ///
    /// A rebuild that fails carries the previous index forward as
    /// [`Reindex::Stale`] rather than swallowing the failure, so the reload can
    /// publish the new catalog with the old index and surface the staleness. A
    /// stale shortlist is a name `run_prompt` will correct; no shortlist at all
    /// is a tool that stopped working over one bad save.
    #[must_use]
    pub(crate) fn rebuilt(&self, catalog: &Catalog) -> Reindex {
        self.refreshed(catalog)
    }

    /// The retrieval a rebuild produces, without the outcome around it.
    ///
    /// The reindex tests assert over the index itself - what it ranks and that
    /// it is a new value rather than a mutation - so they take the retrieval
    /// directly. Production goes through [`rebuilt`](Retrieval::rebuilt), which
    /// reports whether the rebuild fell back to a stale index.
    #[cfg(all(test, feature = "picker"))]
    #[must_use]
    pub(crate) fn reindexed(&self, catalog: &Catalog) -> Retrieval {
        self.rebuilt(catalog).into_retrieval()
    }

    /// Whether a `need_prompt` call would be answered with candidates.
    #[must_use]
    pub(crate) fn is_available(&self) -> bool {
        self.available()
    }

    /// Builds the index over `catalog`, or reports why it could not.
    #[cfg(feature = "picker")]
    fn built(catalog: &Catalog) -> Retrieval {
        match index::Index::build(catalog) {
            Some(index) => {
                tracing::info!("need_prompt ranks {} prompt(s)", index.len());
                Retrieval {
                    index: Some(Arc::new(index)),
                }
            }
            None => Retrieval::idle(),
        }
    }

    /// Without the ranking engine there is nothing to build.
    #[cfg(not(feature = "picker"))]
    fn built(_catalog: &Catalog) -> Retrieval {
        tracing::info!("this build has no picker feature: need_prompt is not published");
        Retrieval::idle()
    }

    /// Ranks a capability against the held index.
    #[cfg(feature = "picker")]
    fn ranked(&self, capability: &str) -> Shortlist {
        match &self.index {
            Some(index) => index.shortlist(capability, CANDIDATES),
            None => Shortlist::Unavailable,
        }
    }

    /// A fresh retrieval over `catalog`, sharing this one's model.
    #[cfg(feature = "picker")]
    fn refreshed(&self, catalog: &Catalog) -> Reindex {
        let Some(current) = &self.index else {
            return Reindex::Unchanged(Retrieval::idle());
        };
        match current.rebuild(catalog) {
            Some(rebuilt) => {
                tracing::info!("need_prompt now ranks {} prompt(s)", rebuilt.len());
                Reindex::Rebuilt(Retrieval {
                    index: Some(Arc::new(rebuilt)),
                })
            }
            None => Reindex::Stale(self.clone()),
        }
    }

    /// Without the ranking engine there is nothing to refresh.
    #[cfg(not(feature = "picker"))]
    #[expect(
        clippy::unused_self,
        reason = "mirrors the picker-enabled method's signature so the caller needs no cfg"
    )]
    fn refreshed(&self, _catalog: &Catalog) -> Reindex {
        Reindex::Unchanged(Retrieval::idle())
    }

    /// Whether an index is held.
    #[cfg(feature = "picker")]
    fn available(&self) -> bool {
        self.index.is_some()
    }

    /// Without the ranking engine retrieval is never available.
    #[cfg(not(feature = "picker"))]
    #[expect(
        clippy::unused_self,
        reason = "mirrors the picker-enabled method's signature so the caller needs no cfg"
    )]
    fn available(&self) -> bool {
        false
    }

    /// Indexes `catalog` with an already-loaded model, so the test binary pays
    /// for the weights once however many indexes it builds.
    #[cfg(all(test, feature = "picker"))]
    pub(crate) fn indexed_with(
        model: &promptforge_tool_picker::Model,
        catalog: &Catalog,
    ) -> Retrieval {
        match index::Index::build_with(model, catalog) {
            Some(index) => Retrieval {
                index: Some(Arc::new(index)),
            },
            None => Retrieval::idle(),
        }
    }

    /// Whether two retrievals share the very same built index, which is what a
    /// test asserts a body-only edit preserved rather than rebuilt.
    #[cfg(all(test, feature = "picker"))]
    pub(crate) fn same_index(&self, other: &Retrieval) -> bool {
        match (&self.index, &other.index) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

/// What rebuilding retrieval over a reloaded catalog produced.
///
/// A rebuild reuses the already-loaded model, so it can fail on its own even
/// when the catalog resolved cleanly. Every arm carries a [`Retrieval`] the
/// reload publishes beside the new catalog: an index always rides the swap, and
/// the arm tells the reload whether that index is fresh or the previous one held
/// over. Minimal on purpose - the reload reads only [`Reindex::is_stale`] and
/// takes the retrieval - so retrieval keeps owning how an index is built while
/// the reload owns what a failure means for the published generation.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum Reindex {
    /// A fresh index over the reloaded catalog.
    #[cfg_attr(
        not(feature = "picker"),
        expect(
            dead_code,
            reason = "only a picker build has an index to rebuild; the arm still exists so reload needs no cfg"
        )
    )]
    Rebuilt(Retrieval),
    /// The rebuild failed; the previous index rides forward and is stale until
    /// the next successful reload.
    #[cfg_attr(
        not(feature = "picker"),
        expect(
            dead_code,
            reason = "only a picker build can fail a rebuild; the arm still exists so reload needs no cfg"
        )
    )]
    Stale(Retrieval),
    /// There was no live index to rebuild a fresh one from, so nothing changed.
    Unchanged(Retrieval),
}

impl Reindex {
    /// The retrieval to publish, whichever arm this is.
    #[must_use]
    pub(crate) fn into_retrieval(self) -> Retrieval {
        match self {
            Reindex::Rebuilt(retrieval)
            | Reindex::Stale(retrieval)
            | Reindex::Unchanged(retrieval) => retrieval,
        }
    }

    /// Whether the published index is the previous one, carried forward because
    /// the rebuild failed.
    #[must_use]
    pub(crate) fn is_stale(&self) -> bool {
        matches!(self, Reindex::Stale(_))
    }
}

/// The index is shared across every handler clone and rebuilt from the watcher,
/// so it must cross threads and outlive any one request. A regression that made
/// it otherwise would surface here rather than at a distant `spawn`.
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Retrieval>();
};

impl Default for Retrieval {
    /// An idle index, which answers nothing.
    fn default() -> Retrieval {
        Retrieval::idle()
    }
}

impl std::fmt::Debug for Retrieval {
    /// Reports whether retrieval can answer, never the vectors behind it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retrieval")
            .field("available", &self.is_available())
            .finish_non_exhaustive()
    }
}
