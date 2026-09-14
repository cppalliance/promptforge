//! The deployment environment: [`Environment`].

use std::fmt;
use std::sync::Arc;

use promptforge_parser::ModelKeyword;
use promptforge_tool_picker::ToolPicker;
use shared_promptforge_api::capabilities::{CapabilityId, RunServices};

use crate::capabilities::CapabilityRegistry;
use crate::client::GatewayClient;
use crate::model::{ModelCatalog, ThinkingMode};
use crate::parser::Prompt;
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::RunResult;
use super::bindings::ModelBindings;
use super::config::{RunContext, RunResolution};
use super::requirements::{RequirementCheck, Requirements, UnmetRequirement};

/// What exists in this deployment and its standing policy.
///
/// Safe to share across concurrent [`run`](Environment::run) calls
/// (`Sync`); built once per host and never rebuilt: everything that can
/// change per run rides the [`RunContext`]. Model-free: the gateway's
/// model list is a host-UI concern and never crosses this interface.
///
/// Interim state (the interface consolidation step): the environment
/// absorbs the retired resolution context's contents - the picker, the
/// model catalog, and the tool catalog - as internal fields, and prose
/// binding still works. [`prepare`](Environment::prepare) installs those
/// inputs on the context, resolves the prompt's declared capabilities
/// against the registry, builds the per-run router from `base_vfs`, and
/// fills the model bindings from the context's current model; the
/// `max_depth` guard lands with the sub-run adapter in the deferred
/// prompt-pack work and is carried, not consulted, until then.
#[non_exhaustive]
pub struct Environment {
    /// Semantic picker behind executed H1 binds (interim home, absorbed
    /// from the retired resolution context).
    picker: Option<Arc<ToolPicker>>,
    /// Live model catalog behind executed H1 model calls (interim home).
    models: ModelCatalog,
    /// Tool catalog behind executed H1 `tools.bind` calls (interim home).
    tools: ToolCatalog,
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
    /// Builds the default environment: no picker, empty model and tool
    /// catalogs, no client, no host roots, and a nesting cap of 3.
    #[must_use]
    pub fn new() -> Environment {
        Environment {
            picker: None,
            models: ModelCatalog::default(),
            tools: ToolCatalog::default(),
            client: None,
            registry: None,
            base_vfs: VfsRef::builder().build(),
            max_depth: 3,
        }
    }

    /// Sets the semantic picker executed H1 binds resolve through.
    #[must_use]
    pub fn picker(mut self, picker: ToolPicker) -> Environment {
        self.picker = Some(Arc::new(picker));
        self
    }

    /// Sets the live model catalog executed H1 model calls resolve against.
    #[must_use]
    pub fn models(mut self, models: ModelCatalog) -> Environment {
        self.models = models;
        self
    }

    /// Sets the tool catalog executed H1 `tools.bind` calls resolve against.
    #[must_use]
    pub fn tools(mut self, tools: ToolCatalog) -> Environment {
        self.tools = tools;
        self
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
    /// declarations: installs the environment's live resolution inputs and
    /// client default, builds the run's VFS, activates every declared
    /// capability, and fills the model bindings - reporting what the
    /// caller must still satisfy.
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
    /// is skipped with a log line. Each present capability is activated
    /// with the run's services (its VFS and cancellation handle); an
    /// activation failure is logged and the capability contributes
    /// nothing to the run - and when the failed capability is required,
    /// it also lands in [`Requirements::missing_required`], since the
    /// run cannot have what the prompt declared. The contributions ride
    /// the context for the catalog-assembly step.
    ///
    /// Model satisfaction is a fill function over the declared roles, and
    /// v1's fill is deliberately trivial: every role binds to the
    /// context's current model, and each role's hard keywords
    /// (`thinking`, `no-thinking`) and context minimum are CHECKED
    /// against its descriptor - reported in
    /// [`Requirements::unmet_requirements`] with required versus actual,
    /// never shopped for. Soft keywords document author intent. With no
    /// current model there is nothing to fill or check, and the interim
    /// Lua-side catalog resolution carries the run.
    pub fn prepare(&self, prompt: &Prompt, ctx: RunContext) -> (RunContext, Requirements) {
        let mut ctx = ctx;
        // The interim resolution inputs (picker, live catalogs) ride the
        // environment until prose binding leaves the run path; prepare
        // installs them so a prepared context is fully equipped whether
        // the host drives the free run itself or goes through
        // [`run`](Environment::run).
        ctx.resolution = Some(RunResolution {
            picker: self.picker.clone(),
            models: self.models.clone(),
            tools: self.tools.clone(),
        });
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
        let services = RunServices::new(ctx.vfs.clone(), ctx.cancel.clone().unwrap_or_default());
        let mut requirements = Requirements::default();
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
            match capability.create(&services) {
                Ok(contribution) => {
                    tracing::info!(capability = %id, "capability activated");
                    ctx.contributions.push(contribution);
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
                    if !declaration.is_optional() {
                        requirements.missing_required.push(id);
                    }
                }
            }
        }
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

/// v1's deliberately trivial fill: binds every declared role to the
/// context's current model and checks each role's hard keywords and
/// context minimum against its descriptor, reporting required versus
/// actual into [`Requirements::unmet_requirements`]. With no current
/// model there is nothing to fill or check.
fn fill_model_bindings(
    prompt: &Prompt,
    model: Option<&crate::model::ModelDescriptor>,
    requirements: &mut Requirements,
) -> ModelBindings {
    let mut bindings = ModelBindings::default();
    let Some(model) = model else {
        return bindings;
    };
    for (label, role) in prompt.frontmatter().models().iter() {
        if let Some(minimum) = role.min_context()
            && model.context() < minimum
        {
            requirements.unmet_requirements.push(UnmetRequirement {
                role: label.to_owned(),
                check: RequirementCheck::ContextMinimum,
                required: minimum.to_string(),
                actual: model.context().to_string(),
            });
        }
        for keyword in role.keywords() {
            // Soft keywords document author intent; only the hard
            // keywords have a descriptor property to check against.
            let failed = match keyword {
                ModelKeyword::Thinking if model.thinking() == ThinkingMode::Never => {
                    Some("thinking")
                }
                ModelKeyword::NoThinking if model.thinking() != ThinkingMode::Never => {
                    Some("no-thinking")
                }
                _ => None,
            };
            if let Some(required) = failed {
                requirements.unmet_requirements.push(UnmetRequirement {
                    role: label.to_owned(),
                    check: RequirementCheck::HardKeyword,
                    required: required.to_owned(),
                    actual: thinking_name(model.thinking()).to_owned(),
                });
            }
        }
        bindings.bind(label, model.clone());
    }
    bindings
}

/// The thinking capability as a stable word for required-versus-actual
/// reporting.
fn thinking_name(thinking: ThinkingMode) -> &'static str {
    match thinking {
        ThinkingMode::Never => "Never",
        ThinkingMode::Always => "Always",
        ThinkingMode::Switchable => "Switchable",
        // The vocabulary is closed today; a future mode reports as
        // unknown rather than breaking the report.
        _ => "unknown",
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
            .field("picker", &self.picker.is_some())
            .field("models", &self.models)
            .field("tools", &"<ToolCatalog>")
            .field("client", &self.client)
            .field("registry", &self.registry)
            .field("base_vfs", &self.base_vfs)
            .field("max_depth", &self.max_depth)
            .finish()
    }
}
