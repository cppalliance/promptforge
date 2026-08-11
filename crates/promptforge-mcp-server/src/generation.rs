//! One live generation: a resolved catalog and the retrieval index built over
//! it, bound together so a reader never sees one without the other.
//!
//! The catalog and its retrieval index used to be two independent cells, each
//! swapped on its own. A reload swapped one and then the other, so between the
//! two stores a reader could load a new catalog beside the old index, or the
//! reverse - `need_prompt` offering a name the runner's catalog no longer had,
//! or the runner serving a prompt retrieval could not find. Folding both into
//! one immutable `Generation` behind a single [`ArcSwap`](arc_swap::ArcSwap)
//! makes that torn pair unrepresentable: a reload builds a whole new generation
//! and publishes it in one store, and a reader loads the pair or the older pair,
//! never a mix of the two.
//!
//! A generation is immutable once built. A run or a `need_prompt` call that
//! loaded one finishes under it whatever a concurrent reload publishes, exactly
//! as a run in flight kept its catalog snapshot before.

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::retrieval::Retrieval;
#[cfg(feature = "picker")]
use crate::retrieval::Shortlist;

/// A resolved catalog and the retrieval index built over exactly that catalog.
///
/// The two are published together and read together, so the index a
/// `need_prompt` call ranks against is always the one built over the catalog a
/// `run_prompt` call in the same generation resolves names in.
#[derive(Debug)]
pub(crate) struct Generation {
    /// The prompts the harness may reach, behind an `Arc` so a snapshot handed
    /// to a run in flight outlives the generation it was loaded from.
    catalog: Arc<Catalog>,
    /// The retrieval index over that catalog. Idle when this build has no
    /// `picker` feature or the model would not load.
    retrieval: Retrieval,
}

impl Generation {
    /// Binds a catalog and the retrieval built over it into one generation.
    #[must_use]
    pub(crate) fn new(catalog: Catalog, retrieval: Retrieval) -> Generation {
        Generation {
            catalog: Arc::new(catalog),
            retrieval,
        }
    }

    /// The resolved catalog this generation publishes.
    #[must_use]
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The retrieval index built over [`catalog`](Self::catalog).
    #[must_use]
    pub(crate) fn retrieval(&self) -> &Retrieval {
        &self.retrieval
    }

    /// The prompts closest to `capability`, ranked by this generation's index.
    ///
    /// Blocks for one embedding forward pass, so it belongs on a blocking task -
    /// which is where the handler that answers `need_prompt` calls it. Only a
    /// `picker` build ranks, so this exists only there.
    #[cfg(feature = "picker")]
    #[must_use]
    pub(crate) fn shortlist(&self, capability: &str) -> Shortlist {
        self.retrieval.shortlist(capability)
    }
}

/// A generation is loaded by every handler clone and rebuilt from the watcher,
/// so it must cross threads and outlive any one request. A regression that made
/// it otherwise would surface here rather than at a distant `spawn`.
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Generation>();
};
