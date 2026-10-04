//! The deployment environment: [`Environment`].

use std::fmt;

use promptforge_types::capabilities::Prelude;

use crate::parser::Prompt;
use crate::tools::ToolCatalog;

use super::config::RunContext;
use super::fill::{fill_model_bindings, fill_tool_bindings};
use super::requirements::Requirements;

/// The tools and capability preludes that a deployment makes available to
/// its runs.
///
/// An environment holds the catalog of tools the caller has made
/// available and the Lua preludes its activated capabilities contributed.
/// It is safe to share across concurrent runs (it is `Sync`). Everything
/// that can change per run sits on the [`RunContext`]. The catalog holds
/// tool descriptors only. The tool implementations stay with the caller,
/// which activates them. The environment holds no model: the run's model
/// arrives on the context.
///
/// [`prepare`](Environment::prepare) fills a prompt's tool slots by
/// identity against the catalog and binds its model roles to the
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

    /// Sets the catalog of tools that a run may bind.
    ///
    /// The catalog holds the tool descriptors the caller assembled from its
    /// activated capabilities. The caller activates its capabilities and
    /// installs the resulting catalog here before it calls
    /// [`prepare`](Environment::prepare). `prepare` then fills the prompt's
    /// exact slots against the catalog by identity.
    #[must_use]
    pub fn tools(mut self, tools: ToolCatalog) -> Environment {
        self.tools = tools;
        self
    }

    /// Sets the capability preludes that every section VM of a run installs.
    ///
    /// A prelude is Lua source that one of the caller's activated
    /// capabilities contributed. The caller passes them in install order,
    /// which is the order the prompt declares the capabilities.
    /// [`prepare`](Environment::prepare) copies them onto the context.
    /// Every section VM of the run installs each prelude after the Engine
    /// globals and before the shared library replays, so the shared library
    /// can call what the preludes define.
    ///
    /// A prelude fails the run as
    /// [`RunErrorKind::Lua`](super::RunErrorKind::Lua) if it fails to load,
    /// or if it defines a global that another prelude, an Engine global, a
    /// reserved name, or a frontmatter tool or model alias already holds.
    /// The failure happens when the first section VM is set up, before the
    /// run issues any effect.
    #[must_use]
    pub fn preludes(mut self, preludes: Vec<Prelude>) -> Environment {
        self.preludes = preludes;
        self
    }

    /// Prepares the caller's context to run `prompt` and reports what the
    /// caller must still satisfy.
    ///
    /// `prepare` copies the catalog and the preludes onto `ctx`, fills the
    /// prompt's tool slots, and binds its model roles. It returns the
    /// enriched context together with the report. It uses the context's
    /// filesystem as given, because the caller builds the run's whole
    /// filesystem, including its real directories and the declared store.
    ///
    /// Tool slots fill against the catalog. An exact slot fills by
    /// identity, and the first two segments of its path name its
    /// capability. If that capability contributed nothing to the catalog,
    /// the capability is listed in [`Requirements::missing_required`]. If
    /// the capability is in the catalog but did not contribute the named
    /// tool, the slot is left unfilled and not reported. Advertising an
    /// unfilled alias fails at run time. Every fill is recorded in the
    /// context's tool bindings. Capability resolution, co-activation
    /// conflicts, and activation happen in the caller before `prepare`, and
    /// the caller merges that report into the one `prepare` returns.
    ///
    /// Every declared model role binds to the context's current model.
    /// `prepare` then checks each role's hard keywords (`thinking`,
    /// `no-thinking`) and context minimum against the model's descriptor.
    /// Each failed check is listed in [`Requirements::unmet_requirements`]
    /// with the required and actual values. `prepare` never searches for
    /// another model that would pass. Soft keywords record the author's
    /// intent and are not checked. With no current model there is nothing
    /// to fill or check, and the declared roles stay unbound.
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
