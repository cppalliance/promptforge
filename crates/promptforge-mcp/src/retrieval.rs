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
//! The similarity floor is zero. The ranking engine's own default was tuned for
//! author-register prose and abstains on a conversationally phrased need, and a
//! floor exists to stop an unattended binding - which is not what happens here,
//! because a model reads the candidates and decides. Three weak candidates are
//! self-evidently weak to that reader, while an empty answer to a casually
//! phrased request helps nobody.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// The prompt's name, as `run_prompt` and `list_prompts` spell it.
    pub name: String,
    /// The prompt's description, verbatim from its frontmatter.
    pub description: String,
}

/// What one retrieval attempt produced.
///
/// The three arms are three different situations for the caller, which is why
/// they are not collapsed into a `Result<Vec<_>>`: an empty
/// [`Candidates`](Shortlist::Candidates) is a complete answer, and the other two
/// say that no answer was reached and which side the reason sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shortlist {
    /// The prompts closest to the capability, best first, at most three of
    /// them. Empty when the catalog offered none.
    Candidates(Vec<Candidate>),
    /// Retrieval is not part of this build, or its model could not be loaded.
    Unavailable,
    /// The engine could not embed the capability, carrying the reason.
    Failed(String),
}

/// The live retrieval index, rebuilt when a save changes what it ranks on.
///
/// Shared by the handler that answers `need_prompt` and the reload that keeps it
/// current, so both see one index. Replacing it is an atomic swap and a call in
/// flight keeps the index it started with, exactly as a run keeps its catalog
/// snapshot.
pub struct Retrieval {
    /// The engine over the catalog's descriptors, absent until one is built and
    /// absent for good if the model could not be loaded.
    #[cfg(feature = "picker")]
    index: arc_swap::ArcSwapOption<index::Index>,
    /// How many times the index has been replaced, which is what a test asserts
    /// a body-only edit did not do.
    #[cfg(feature = "picker")]
    rebuilds: std::sync::atomic::AtomicUsize,
}

impl Retrieval {
    /// Retrieval that answers nothing, for a server built without an index.
    ///
    /// Every `need_prompt` call against it reports
    /// [`Shortlist::Unavailable`]. This is what a build without the `picker`
    /// feature has, and what a test that is not about retrieval uses.
    #[must_use]
    pub fn idle() -> Retrieval {
        Retrieval {
            #[cfg(feature = "picker")]
            index: arc_swap::ArcSwapOption::empty(),
            #[cfg(feature = "picker")]
            rebuilds: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Builds the index over `catalog`, loading the embedding model once.
    ///
    /// This is the slow call in the server's boot: it parses tens of megabytes
    /// of compiled-in weights and embeds every runnable prompt. It cannot fail.
    /// A model that will not load is reported at error level and the result is
    /// an [idle](Retrieval::idle) index, because the rest of the server - every
    /// prompt, both listing tools, the runner - is unaffected, and a harness
    /// that cannot start its MCP server is worse off than one whose retrieval
    /// tool says it is unavailable.
    ///
    /// # Examples
    /// ```no_run
    /// # use promptforge_mcp::{Catalog, Config, OnBroken, Retrieval};
    /// # fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    /// let catalog = Catalog::resolve(config, OnBroken::Reject)?;
    /// let retrieval = Retrieval::start(&catalog);
    /// # let _ = retrieval;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn start(catalog: &Catalog) -> Retrieval {
        let retrieval = Retrieval::idle();
        retrieval.install(catalog);
        retrieval
    }

    /// The prompts closest to `capability`, best first.
    ///
    /// The capability is embedded by the model the prompts were embedded with
    /// and every prompt is scored against it; no candidate is withheld for
    /// scoring poorly, since the caller is a model that can see for itself how
    /// weak three candidates are.
    ///
    /// This blocks for as long as embedding the capability takes - one forward
    /// pass through the model - so it belongs on a blocking task, which is where
    /// the handler that answers `need_prompt` calls it. The index it ranks
    /// against is taken once, so a rebuild that lands mid-call cannot show a
    /// caller a half-built index.
    #[must_use]
    pub fn shortlist(&self, capability: &str) -> Shortlist {
        self.ranked(capability)
    }

    /// Rebuilds the index over `catalog`, reusing the loaded model.
    ///
    /// The answer to a save that changed a name or a description. It costs one
    /// forward pass per prompt and no weights, which is what lets it ride the
    /// same catalog swap the watcher already performs. An idle index stays idle:
    /// there is no model behind it to reuse, and loading one here would put the
    /// boot's cost on a filesystem event.
    ///
    /// This blocks for as long as the embedding takes, so it belongs on a
    /// blocking task - which is where the reload that calls it already runs.
    ///
    /// A rebuild that fails keeps the previous index and logs the reason. A
    /// stale shortlist is a name `run_prompt` will correct; no shortlist at all
    /// is a tool that stopped working over one bad save.
    pub fn rebuild(&self, catalog: &Catalog) {
        self.refresh(catalog);
    }

    /// Whether a `need_prompt` call would be answered with candidates.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.available()
    }

    /// Builds the index and stores it, or reports why it could not.
    #[cfg(feature = "picker")]
    fn install(&self, catalog: &Catalog) {
        if let Some(index) = index::Index::build(catalog) {
            tracing::info!("need_prompt ranks {} prompt(s)", index.len());
            self.index.store(Some(std::sync::Arc::new(index)));
        }
    }

    /// Without the ranking engine there is nothing to install.
    #[cfg(not(feature = "picker"))]
    #[expect(
        clippy::unused_self,
        reason = "mirrors the picker-enabled method's signature so the caller needs no cfg"
    )]
    fn install(&self, _catalog: &Catalog) {
        tracing::info!("this build has no picker feature: need_prompt is not published");
    }

    /// Ranks a capability against the stored index.
    #[cfg(feature = "picker")]
    fn ranked(&self, capability: &str) -> Shortlist {
        match self.index.load_full() {
            Some(index) => index.shortlist(capability, CANDIDATES),
            None => Shortlist::Unavailable,
        }
    }

    /// Without the ranking engine nothing can be ranked.
    #[cfg(not(feature = "picker"))]
    #[expect(
        clippy::unused_self,
        reason = "mirrors the picker-enabled method's signature so the caller needs no cfg"
    )]
    fn ranked(&self, _capability: &str) -> Shortlist {
        Shortlist::Unavailable
    }

    /// Replaces the index with one over `catalog`, over the same model.
    #[cfg(feature = "picker")]
    fn refresh(&self, catalog: &Catalog) {
        let Some(current) = self.index.load_full() else {
            return;
        };
        if let Some(rebuilt) = current.rebuild(catalog) {
            tracing::info!("need_prompt now ranks {} prompt(s)", rebuilt.len());
            self.index.store(Some(std::sync::Arc::new(rebuilt)));
            let _previous = self
                .rebuilds
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Without the ranking engine there is nothing to refresh.
    #[cfg(not(feature = "picker"))]
    #[expect(
        clippy::unused_self,
        reason = "mirrors the picker-enabled method's signature so the caller needs no cfg"
    )]
    fn refresh(&self, _catalog: &Catalog) {}

    /// Whether an index is loaded.
    #[cfg(feature = "picker")]
    fn available(&self) -> bool {
        self.index.load().is_some()
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
    ///
    /// An instance method rather than a constructor, because a test hands the
    /// index to a server and a reload before it has a catalog to index.
    #[cfg(all(test, feature = "picker"))]
    pub(crate) fn install_with(
        &self,
        embedder: std::sync::Arc<promptforge_tool_picker::Embedder>,
        catalog: &Catalog,
    ) {
        if let Some(index) = index::Index::build_with(embedder, catalog) {
            self.index.store(Some(std::sync::Arc::new(index)));
        }
    }

    /// How many times the index has been replaced since it was built.
    #[cfg(all(test, feature = "picker"))]
    pub(crate) fn rebuilds(&self) -> usize {
        self.rebuilds.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for Retrieval {
    /// An [idle](Retrieval::idle) index, which answers nothing.
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
