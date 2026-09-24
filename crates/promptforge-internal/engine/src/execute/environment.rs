//! The deployment environment: [`Environment`].

use std::fmt;

use crate::parser::Prompt;
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::config::RunContext;
use super::fill::{fill_model_bindings, fill_tool_bindings};
use super::requirements::Requirements;

/// What exists in this deployment and its standing policy: the host roots,
/// the nesting cap, and the catalog of tools the host has made available.
///
/// Safe to share across concurrent runs (`Sync`): everything that can
/// change per run sits on the [`RunContext`], and the tool implementations
/// stay with the host (the harness's activation, in
/// `harness-capabilities`). Model-free: the gateway's model list is a
/// host-UI concern and never crosses this interface.
///
/// [`prepare`](Environment::prepare) builds the per-run router from
/// `base_vfs`, fills the prompt's tool slots by identity against the
/// catalog, and fills the model bindings from the context's current model;
/// the `max_depth` guard lands with the sub-run adapter in the deferred
/// prompt-pack work and is stored but not consulted until then.
#[derive(Clone)]
#[non_exhaustive]
pub struct Environment {
    /// Host roots the per-run router mounts at `/`; never includes the
    /// store mount (prepare adds a fresh per-run memory backend there).
    base_vfs: VfsRef,
    /// Maximum model-orchestrated prompt-tool nesting, copied into every
    /// run. Inert until the sub-run adapter lands with the prompt-pack.
    max_depth: u32,
    /// The tools a run may bind, as descriptors: assembled by the host from
    /// its activated capabilities. The default is empty, so every exact
    /// slot's capability is reported missing.
    tools: ToolCatalog,
}

impl Environment {
    /// Builds the default environment: no host roots, a nesting cap of 3,
    /// and an empty catalog.
    #[must_use]
    pub fn new() -> Environment {
        Environment {
            base_vfs: VfsRef::builder().build(),
            max_depth: 3,
            tools: ToolCatalog::default(),
        }
    }

    /// Sets the host roots the per-run router mounts at `/`. Consulted by
    /// [`prepare`](Environment::prepare); the base must hold host roots
    /// only, never the store mount.
    #[must_use]
    pub fn base_vfs(mut self, vfs: VfsRef) -> Environment {
        self.base_vfs = vfs;
        self
    }

    /// Sets the maximum model-orchestrated prompt-tool nesting depth.
    /// Consulted by the sub-run adapter when it lands with the
    /// prompt-pack; stored inert until then.
    #[must_use]
    pub fn max_depth(mut self, max_depth: u32) -> Environment {
        self.max_depth = max_depth;
        self
    }

    /// Sets the catalog of tools a run may bind: the descriptors the host
    /// assembled from its activated capabilities.
    /// [`prepare`](Environment::prepare) fills the prompt's exact slots
    /// against it by identity. A host that activates a registry (the
    /// harness's `activate`) installs the activated catalog here before
    /// preparing.
    #[must_use]
    pub fn tools(mut self, tools: ToolCatalog) -> Environment {
        self.tools = tools;
        self
    }

    /// Builds one run's VFS: a fresh router mounting the environment's
    /// [`base_vfs`](Environment::base_vfs) at `/` plus a fresh memory
    /// backend at the store mount - never an overlay: an overlay shares
    /// the base's claims table, which is only correct for two views of
    /// the same storage, and concurrent runs' stores are different
    /// storage. The shared base's own claims table still catches two
    /// runs conflicting on one host file under the caller's identity.
    ///
    /// [`prepare`](Environment::prepare) builds one unless the host set
    /// the context's handle itself; a host that activates capabilities
    /// builds it here first, hands it to activation's services, and sets
    /// it on the context so the capabilities and the run share one store.
    #[must_use]
    pub fn run_vfs(&self) -> VfsRef {
        VfsRef::builder()
            .mount("/", self.base_vfs.clone())
            .mount(
                promptforge_vfs::STORE_MOUNT,
                shared_vfs::MemoryBackend::new(),
            )
            .build()
    }

    /// Enriches the caller-created context against the prompt's
    /// declarations: builds the run's VFS (unless the host set one),
    /// installs the catalog, and fills the tool slots and model bindings -
    /// reporting what the caller must still satisfy.
    ///
    /// The per-run VFS is [`run_vfs`](Environment::run_vfs): a fresh
    /// router over the shared base with the run's own store.
    ///
    /// Tool slot filling runs against the catalog: exact slots fill by
    /// identity - an exact path's first two segments name its capability,
    /// so a slot whose capability contributed nothing to the catalog lands
    /// in [`Requirements::missing_required`], while a slot whose
    /// capability is in the catalog but contributed no such tool is
    /// warned and left unfilled (advertising an unfilled alias fails at
    /// run time). Every fill is journaled into the context's tool
    /// bindings. Capability resolution, co-activation conflicts, and
    /// activation itself happen before prepare in the host (the harness's
    /// `activate`), which merges that report into this one.
    ///
    /// Model satisfaction is a fill function over the declared roles, and
    /// v1's fill is deliberately trivial: every role binds to the
    /// context's current model, and each role's hard keywords
    /// (`thinking`, `no-thinking`) and context minimum are CHECKED
    /// against its descriptor - reported in
    /// [`Requirements::unmet_requirements`] with required versus actual,
    /// never shopped for. Soft keywords document author intent. With no
    /// current model there is nothing to fill or check, and declared
    /// roles stay unbound.
    #[must_use]
    pub fn prepare(&self, prompt: &Prompt, ctx: RunContext) -> (RunContext, Requirements) {
        let mut ctx = ctx;
        if !ctx.vfs_explicit {
            ctx.vfs = self.run_vfs();
        }
        let mut requirements = Requirements::default();
        ctx.tools = self.tools.clone();
        ctx.tool_bindings = fill_tool_bindings(prompt, &ctx.tools, &mut requirements);
        ctx.model_bindings = fill_model_bindings(prompt, ctx.model.as_ref(), &mut requirements);
        (ctx, requirements)
    }
}

impl Default for Environment {
    fn default() -> Environment {
        Environment::new()
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment")
            .field("base_vfs", &self.base_vfs)
            .field("max_depth", &self.max_depth)
            .field("tools", &self.tools)
            .finish()
    }
}
