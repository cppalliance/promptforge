//! The deployment environment: [`Environment`].

use std::fmt;

use promptforge_api_types::capabilities::RunServices;

use crate::parser::Prompt;
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::RunResult;
use super::config::RunContext;
use super::fill::{fill_model_bindings, fill_tool_bindings};
use super::host::RunHost;
use super::requirements::Requirements;

/// What exists in this deployment and its standing policy: the host roots,
/// the nesting cap, and the catalog of tools the host has made available.
///
/// Safe to share across concurrent runs (`Sync`) and holds nothing live:
/// everything that can change per run rides the [`RunContext`], and the
/// tool implementations stay with the host (see
/// [`activation`](super::activation)). Model-free: the gateway's model
/// list is a host-UI concern and never crosses this interface.
///
/// [`prepare`](Environment::prepare) builds the per-run router from
/// `base_vfs`, fills the prompt's tool slots by identity against the
/// catalog, and fills the model bindings from the context's current model;
/// the `max_depth` guard lands with the sub-run adapter in the deferred
/// prompt-pack work and is carried, not consulted, until then.
#[derive(Clone)]
#[non_exhaustive]
pub struct Environment {
    /// Host roots the per-run router mounts at `/`; never carries the
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
    /// [`prepare`](Environment::prepare); the base must carry host roots
    /// only, never the store mount.
    #[must_use]
    pub fn base_vfs(mut self, vfs: VfsRef) -> Environment {
        self.base_vfs = vfs;
        self
    }

    /// Sets the maximum model-orchestrated prompt-tool nesting depth.
    /// Consulted by the sub-run adapter when it lands with the
    /// prompt-pack; carried inert until then.
    #[must_use]
    pub fn max_depth(mut self, max_depth: u32) -> Environment {
        self.max_depth = max_depth;
        self
    }

    /// Sets the catalog of tools a run may bind: the descriptors the host
    /// assembled from its activated capabilities.
    /// [`prepare`](Environment::prepare) fills the prompt's exact slots
    /// against it by identity. A host running through
    /// [`run`](Environment::run) with a registry on its
    /// [`RunHost`] never sets this: the activated catalog is installed
    /// there.
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
    /// the context's handle itself; [`run`](Environment::run) builds it
    /// here before activating, hands it to activation's services, and
    /// sets it on the context so the capabilities and the run share one
    /// store.
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
    /// activation itself happen before prepare
    /// ([`activation::activate`](super::activation::activate)), on the
    /// loop path in [`run`](Environment::run), which merges that report
    /// into this one.
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

    /// The zero-burden path: activates, [prepares](Environment::prepare),
    /// and runs.
    ///
    /// When `host` carries a [registry](RunHost::registry), this is the
    /// one place the prompt's declared capabilities activate: the run's
    /// VFS is built first ([`run_vfs`](Environment::run_vfs), unless the
    /// host set the context's handle itself) so the capabilities'
    /// services and the run share one store; each declaration activates
    /// with those services ([`activation::activate`](super::activation::activate));
    /// the activated catalog is the run's catalog, its implementations
    /// go to the loop's tool performer, and what activation could not
    /// satisfy is folded into prepare's report. Without a registry
    /// nothing activates and the environment's own catalog stands.
    ///
    /// An unsatisfiable prompt - missing required capabilities,
    /// conflicts, or unmet model requirements - is refused with
    /// [`RunResult::Failure`] carrying
    /// [`RequirementsUnmet`](crate::RunErrorKind::RequirementsUnmet) and
    /// a model-readable notice naming each gap once. The notice may
    /// arrive as tool output when the prompt runs as a sub-run tool, so
    /// it is written for a model to reason about.
    #[must_use]
    pub async fn run(
        &self,
        prompt: &Prompt,
        args: &str,
        ctx: RunContext,
        host: RunHost,
    ) -> RunResult {
        let mut ctx = ctx;
        let mut host = host;
        let mut env = self.clone();
        if let Some(registry) = host.registry.take() {
            if !ctx.vfs_explicit {
                ctx = ctx.vfs(self.run_vfs());
            }
            let services = RunServices::new(ctx.vfs.clone(), ctx.cancel_handle());
            let mut activation = super::activation::activate(Some(&registry), prompt, &services);
            env.tools = std::mem::take(&mut activation.catalog);
            host = host.activated(activation);
        }
        let (ctx, mut requirements) = env.prepare(prompt, ctx);
        requirements.merge(host.requirements.clone());
        if !requirements.is_satisfied() {
            return RunResult::Failure(crate::RunError::from(crate::Error::RequirementsUnmet {
                notice: requirements.notice(),
            }));
        }
        super::run(prompt, args, ctx, host).await
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
