//! The deployment environment: [`Environment`].

use std::fmt;

use promptforge_types::capabilities::Prelude;

use crate::parser::Prompt;
use crate::tools::ToolCatalog;

use super::config::RunContext;
use super::fill::{fill_model_bindings, fill_tool_bindings};
use super::requirements::Requirements;

/// What exists in this deployment: the catalog of tools the Harness has
/// made available and the preludes its activated capabilities
/// contributed.
///
/// Safe to share across concurrent runs (`Sync`): everything that can
/// change per run sits on the [`RunContext`], and the tool implementations
/// stay with the Harness, which activates them in
/// `harness-capabilities`. Model-free: the gateway's model list serves the
/// Host's model selection, and the run's model arrives on the context.
///
/// [`prepare`](Environment::prepare) fills the prompt's tool slots by
/// identity against the catalog and fills the model bindings from the
/// context's current model.
#[derive(Clone)]
#[non_exhaustive]
pub struct Environment {
    /// The tools a run may bind, as descriptors: assembled by the Harness from
    /// its activated capabilities. The default is empty, so every exact
    /// slot's capability is reported missing.
    tools: ToolCatalog,
    /// The Lua source the Harness's activated capabilities contributed, in
    /// install order: every section VM of a run installs each one before
    /// the shared library replays. The default is empty.
    preludes: Vec<Prelude>,
}

impl Environment {
    /// Builds the default environment: an empty catalog and no preludes.
    #[must_use]
    pub fn new() -> Environment {
        Environment {
            tools: ToolCatalog::default(),
            preludes: Vec::new(),
        }
    }

    /// Sets the catalog of tools a run may bind: the descriptors the Harness
    /// assembled from its activated capabilities.
    /// [`prepare`](Environment::prepare) fills the prompt's exact slots
    /// against it by identity. The Harness activates its registry (the
    /// `activate` function) and installs the activated catalog here before
    /// preparing.
    #[must_use]
    pub fn tools(mut self, tools: ToolCatalog) -> Environment {
        self.tools = tools;
        self
    }

    /// Sets the preludes a run installs: the Lua source the Harness's
    /// activated capabilities contributed, in install order (the order the
    /// prompt declares the capabilities). [`prepare`](Environment::prepare)
    /// copies them onto the context, and every section VM of the run
    /// installs each one after the Engine globals and before the shared
    /// library replays, so the shared library can call what they define.
    ///
    /// A prelude that fails to load, or that defines a global another
    /// prelude, an Engine global, a reserved name, or a frontmatter tool or
    /// model alias already holds, fails the run as
    /// [`RunErrorKind::Lua`](super::RunErrorKind::Lua) when the first
    /// section VM is set up, before the run issues any effect.
    #[must_use]
    pub fn preludes(mut self, preludes: Vec<Prelude>) -> Environment {
        self.preludes = preludes;
        self
    }

    /// Enriches the caller-created context against the prompt's
    /// declarations: installs the catalog and the preludes and fills the
    /// tool slots and model bindings - reporting what the caller must
    /// still satisfy. The
    /// context's VFS is used as given: the Harness builds the run's whole
    /// filesystem, real directories and the declared store included.
    ///
    /// Tool slot filling runs against the catalog: exact slots fill by
    /// identity - an exact path's first two segments name its capability,
    /// so a slot whose capability contributed nothing to the catalog lands
    /// in [`Requirements::missing_required`], while a slot whose
    /// capability is in the catalog but contributed no such tool is
    /// warned and left unfilled (advertising an unfilled alias fails at
    /// run time). Every fill is journaled into the context's tool
    /// bindings. Capability resolution, co-activation conflicts, and
    /// activation itself happen before prepare, in the Harness's
    /// `activate`, and the Harness merges that report into this one.
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
        let mut requirements = Requirements::default();
        ctx.tools = self.tools.clone();
        ctx.preludes.clone_from(&self.preludes);
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
            .field("tools", &self.tools)
            .field(
                "preludes",
                &self
                    .preludes
                    .iter()
                    .map(Prelude::capability)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
