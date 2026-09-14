//! The deployment environment: [`Environment`].

use std::fmt;
use std::sync::Arc;

use promptforge_tool_picker::ToolPicker;

use crate::client::GatewayClient;
use crate::model::ModelCatalog;
use crate::parser::Prompt;
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::RunResult;
use super::config::{RunContext, RunResolution};

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
/// binding still works. The registry slot, the per-run router built from
/// `base_vfs`, and the `max_depth` guard all land with the prepare pass
/// in the capability-binding work; until then `base_vfs` and `max_depth`
/// are carried, not consulted.
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
    /// Host roots the per-run router mounts; never carries the store
    /// mount. Inert until the prepare pass lands.
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

    /// Sets the host roots the per-run router mounts. Consulted by the
    /// prepare pass when it lands; carried inert until then.
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

    /// The zero-burden path: installs the environment's live resolution
    /// inputs and client default on the context - the interim stand-in for
    /// the prepare pass, which will also fail unsatisfiable requirements
    /// here - and runs the prompt.
    pub async fn run(&self, prompt: &Prompt, args: &str, ctx: RunContext) -> RunResult {
        let mut ctx = ctx;
        ctx.resolution = Some(RunResolution {
            picker: self.picker.clone(),
            models: self.models.clone(),
            tools: self.tools.clone(),
        });
        if ctx.client.is_none() {
            ctx.client = self.client.clone();
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
            .field("picker", &self.picker.is_some())
            .field("models", &self.models)
            .field("tools", &"<ToolCatalog>")
            .field("client", &self.client)
            .field("base_vfs", &self.base_vfs)
            .field("max_depth", &self.max_depth)
            .finish()
    }
}
