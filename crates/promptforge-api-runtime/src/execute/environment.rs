//! The deployment environment: [`Environment`].

use std::fmt;
use std::sync::Arc;

use promptforge_api_types::capabilities::{Capability, CapabilityId, Contribution, RunServices};

use crate::capabilities::CapabilityRegistry;
use crate::client::GatewayClient;
use crate::parser::Prompt;
use crate::store::VfsRef;

use super::RunResult;
use super::config::RunContext;
use super::fill::{assemble_catalog, fill_model_bindings, fill_tool_bindings};
use super::requirements::{CapabilityConflict, Requirements};

/// What exists in this deployment and its standing policy.
///
/// Safe to share across concurrent [`run`](Environment::run) calls
/// (`Sync`); built once per host and never rebuilt: everything that can
/// change per run rides the [`RunContext`]. Model-free: the gateway's
/// model list is a host-UI concern and never crosses this interface.
///
/// [`prepare`](Environment::prepare) resolves the prompt's declared
/// capabilities against the registry (rejecting co-activation conflicts),
/// assembles the activated contributions into the run's tool catalog,
/// builds the per-run router from `base_vfs`, fills the tool slots against
/// the assembled catalog, and fills the model bindings from the context's
/// current model; the `max_depth` guard lands with the sub-run adapter in
/// the deferred prompt-pack work and is carried, not consulted, until then.
#[non_exhaustive]
pub struct Environment {
    /// The deployment's gateway client; a run's own client overrides it.
    client: Option<GatewayClient>,
    /// The explicit host-built set of installed capabilities a prompt's
    /// frontmatter declarations resolve against at prepare.
    registry: Option<CapabilityRegistry>,
    /// Host roots the per-run router mounts at `/`; never carries the
    /// store mount (prepare adds a fresh per-run memory backend there).
    base_vfs: VfsRef,
    /// Maximum model-orchestrated prompt-tool nesting, copied into every
    /// run. Inert until the sub-run adapter lands with the prompt-pack.
    max_depth: u32,
}

impl Environment {
    /// Builds the default environment: no client, no registry, no host
    /// roots, and a nesting cap of 3.
    #[must_use]
    pub fn new() -> Environment {
        Environment {
            client: None,
            registry: None,
            base_vfs: VfsRef::builder().build(),
            max_depth: 3,
        }
    }

    /// Sets the deployment's gateway client; a run's own client overrides
    /// it, and with neither, one is built from the process environment on
    /// first use.
    #[must_use]
    pub fn client(mut self, client: GatewayClient) -> Environment {
        self.client = Some(client);
        self
    }

    /// Sets the deployment's capability registry: the explicit host-built
    /// set of installed capabilities a prompt's frontmatter declarations
    /// resolve against at [`prepare`](Environment::prepare). The default
    /// (`None`) resolves every declared capability as absent.
    #[must_use]
    pub fn registry(mut self, registry: CapabilityRegistry) -> Environment {
        self.registry = Some(registry);
        self
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

    /// Enriches the caller-created context against the prompt's
    /// declarations: installs the environment's client default, builds the
    /// run's VFS, activates every declared capability, assembles the run's
    /// tool catalog, and fills the tool slots and model bindings -
    /// reporting what the caller must still satisfy.
    ///
    /// The per-run VFS is a fresh router mounting the environment's
    /// [`base_vfs`](Environment::base_vfs) at `/` plus a fresh memory
    /// backend at the store mount - never an overlay: an overlay shares
    /// the base's claims table, which is only correct for two views of
    /// the same storage, and concurrent runs' stores are different
    /// storage. The shared base's own claims table still catches two
    /// runs conflicting on one host file under the caller's identity.
    ///
    /// Declared capabilities resolve against the registry in declaration
    /// order. A missing required capability lands in
    /// [`Requirements::missing_required`]; an absent optional capability
    /// is skipped with a log line. Present capabilities are checked for
    /// co-activation conflicts (bashkit vs terminal: two filesystem
    /// realities, and a context gets one or the other, never both); a
    /// conflicting pair activates neither member and lands in
    /// [`Requirements::conflicts`] naming both. Each remaining capability
    /// is activated with the run's services (its VFS and cancellation
    /// handle); an activation failure is logged and the capability
    /// contributes nothing to the run - and when the failed capability
    /// is required, it also lands in [`Requirements::missing_required`],
    /// since the run cannot have what the prompt declared. The activated
    /// contributions are assembled into the run's tool catalog in
    /// declaration order, with tool prefix-containment enforced at
    /// assembly: a contributed tool whose id escapes its capability's id
    /// is rejected - logged and never admitted to the catalog.
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
    ///
    /// Tool slot filling follows catalog assembly: exact slots fill by
    /// identity against the run's catalog - an exact path's first two
    /// segments name its capability, so a slot whose capability is
    /// inactive lands in [`Requirements::missing_required`], while a
    /// slot whose capability is active but contributed no such tool is
    /// warned and left unfilled (advertising an unfilled alias fails at
    /// run time). Every fill is journaled into the context's tool
    /// bindings.
    pub fn prepare(&self, prompt: &Prompt, ctx: RunContext) -> (RunContext, Requirements) {
        let mut ctx = ctx;
        if ctx.client.is_none() {
            ctx.client.clone_from(&self.client);
        }
        ctx.vfs = VfsRef::builder()
            .mount("/", self.base_vfs.clone())
            .mount(
                promptforge_vfs::STORE_MOUNT,
                shared_vfs::MemoryBackend::new(),
            )
            .build();
        let services = RunServices::new(ctx.vfs.clone(), ctx.cancel.clone());
        let mut requirements = Requirements::default();
        // Resolve the declarations against the registry, preserving
        // declaration order.
        let mut present: Vec<(CapabilityId, Arc<dyn Capability>, bool)> = Vec::new();
        for declaration in prompt.frontmatter().capabilities() {
            // The parser validated the id's arity and charset at parse
            // time, so the checked constructor's validation cannot fail.
            let id = CapabilityId::from_validated(&declaration.id().to_string());
            let capability = self
                .registry
                .as_ref()
                .and_then(|registry| registry.get(&id));
            let Some(capability) = capability else {
                if declaration.is_optional() {
                    tracing::info!(capability = %id, "optional capability absent; skipped");
                } else {
                    requirements.missing_required.push(id);
                }
                continue;
            };
            present.push((id, Arc::clone(capability), declaration.is_optional()));
        }
        // Co-activation conflicts are declared by the capabilities
        // themselves; the check is symmetric, so only one member of a
        // pair needs to name the other. A conflicting pair activates
        // neither member and fails preparation naming both.
        let mut conflicted = vec![false; present.len()];
        for (i, (first_id, first, _)) in present.iter().enumerate() {
            for (j, (second_id, second, _)) in present.iter().enumerate().skip(i + 1) {
                if first.conflicts().contains(second_id) || second.conflicts().contains(first_id) {
                    tracing::warn!(
                        first = %first_id,
                        second = %second_id,
                        "conflicting capabilities declared; neither activates"
                    );
                    requirements.conflicts.push(CapabilityConflict {
                        first: first_id.clone(),
                        second: second_id.clone(),
                    });
                    conflicted[i] = true;
                    conflicted[j] = true;
                }
            }
        }
        let mut activated: Vec<(CapabilityId, Contribution)> = Vec::new();
        for ((id, capability, optional), is_conflicted) in
            present.iter().zip(conflicted.iter().copied())
        {
            if is_conflicted {
                continue;
            }
            match capability.create(&services) {
                Ok(contribution) => {
                    tracing::info!(capability = %id, "capability activated");
                    activated.push((id.clone(), contribution));
                }
                Err(error) => {
                    tracing::warn!(
                        capability = %id,
                        %error,
                        "capability activation failed; it contributes nothing to the run"
                    );
                    // A required capability that cannot activate leaves
                    // the run without something the prompt declared:
                    // report it like an absent one so the run fails
                    // until satisfied.
                    if !*optional {
                        requirements.missing_required.push(id.clone());
                    }
                }
            }
        }
        ctx.tools = assemble_catalog(&activated);
        let activated_ids: Vec<CapabilityId> = activated.iter().map(|(id, _)| id.clone()).collect();
        ctx.tool_bindings =
            fill_tool_bindings(prompt, &ctx.tools, &activated_ids, &mut requirements);
        ctx.model_bindings = fill_model_bindings(prompt, ctx.model.as_ref(), &mut requirements);
        (ctx, requirements)
    }

    /// The zero-burden path: [prepares](Environment::prepare) implicitly
    /// and refuses an unsatisfiable prompt - missing required
    /// capabilities, or unmet model requirements - with
    /// [`RunResult::Failure`] carrying
    /// [`RequirementsUnmet`](crate::RunErrorKind::RequirementsUnmet) and
    /// a model-readable notice naming each gap. The notice may arrive as
    /// tool output when the prompt runs as a sub-run tool, so it is
    /// written for a model to reason about.
    pub async fn run(&self, prompt: &Prompt, args: &str, ctx: RunContext) -> RunResult {
        let (ctx, requirements) = self.prepare(prompt, ctx);
        if !requirements.is_satisfied() {
            return RunResult::Failure(crate::RunError::from(crate::Error::RequirementsUnmet {
                notice: requirements.notice(),
            }));
        }
        super::run(prompt, args, ctx).await
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
            .field("client", &self.client)
            .field("registry", &self.registry.is_some())
            .field("base_vfs", &self.base_vfs)
            .field("max_depth", &self.max_depth)
            .finish()
    }
}
