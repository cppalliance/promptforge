//! The deployment environment: [`Environment`].

use std::collections::BTreeMap;
use std::fmt;

use promptforge_types::plugins::{PluginId, Prelude};

use crate::parser::Prompt;
use crate::tools::ToolCatalog;

use super::config::RunContext;
use super::fill::fill_model_bindings;
use super::requirements::{Requirements, UnavailablePlugin};

/// The tools, Plugin roster, and Plugin preludes that a deployment makes
/// available to its runs.
///
/// An environment holds the catalog of tools the caller has made available,
/// the roster of installed Plugins with the reason each one that can't
/// serve gives, and the Lua preludes of the Plugins the prompt declares.
/// It is safe to share across concurrent runs (it is `Sync`). Everything
/// that can change per run sits on the [`RunContext`]. The catalog holds
/// tool descriptors only. The tool implementations stay with the caller,
/// which performs every call. The run's model arrives on the context.
///
/// [`prepare`](Environment::prepare) binds a prompt's model roles to the
/// context's current model. The run offers every catalog tool to the
/// prompt's Lua, which names each by its canonical id.
#[derive(Clone)]
#[non_exhaustive]
pub struct Environment {
    /// The tools a run may be offered, as descriptors: every tool of every
    /// Plugin the caller can serve, declared by the prompt or not. The
    /// default is empty.
    tools: ToolCatalog,
    /// Every installed Plugin the caller reports, by name: no reason when
    /// it can serve the run, and the reason, written for the model, when it
    /// can't. The default is empty.
    roster: BTreeMap<PluginId, Option<String>>,
    /// The Lua source of the Plugins the prompt declares, in declaration
    /// order: every section VM of a run installs each one before `require`.
    /// The default is empty.
    preludes: Vec<Prelude>,
}

impl Environment {
    /// Builds the default environment: an empty catalog, an empty roster,
    /// and an empty prelude list.
    #[must_use]
    pub fn new() -> Environment {
        Environment {
            tools: ToolCatalog::default(),
            roster: BTreeMap::new(),
            preludes: Vec::new(),
        }
    }

    /// Sets the catalog of tools that a run may be offered.
    ///
    /// The catalog holds the tool descriptors of every Plugin the caller
    /// can serve, whether or not the prompt declares it. The caller
    /// installs it here before it calls [`prepare`](Environment::prepare),
    /// which copies it onto the context, and the run offers every tool in
    /// it.
    #[must_use]
    pub fn tools(mut self, tools: ToolCatalog) -> Environment {
        self.tools = tools;
        self
    }

    /// Sets the run's roster: every installed Plugin that can serve the
    /// run, declared by the prompt or not, and every installed Plugin that
    /// can't, with the reason.
    ///
    /// [`prepare`](Environment::prepare) copies the roster onto the
    /// context. Each usable Plugin gets an object in the run's Lua, with or
    /// without tools. An unavailable one gets none, and offering it raises
    /// its reason. Without a roster, the run's Lua sees the declared
    /// Plugins and the Plugins with a tool in the catalog.
    #[must_use]
    pub fn plugins(
        mut self,
        usable: Vec<PluginId>,
        unavailable: Vec<UnavailablePlugin>,
    ) -> Environment {
        self.roster = usable
            .into_iter()
            .map(|plugin| (plugin, None))
            .chain(
                unavailable
                    .into_iter()
                    .map(|entry| (entry.plugin, Some(entry.reason))),
            )
            .collect();
        self
    }

    /// Sets the Plugin preludes that every section VM of a run installs.
    ///
    /// A prelude is Lua source that a Plugin the prompt declares
    /// contributed. The caller passes them in the order the prompt
    /// declares the Plugins.
    /// [`prepare`](Environment::prepare) copies them onto the context.
    /// Every section VM of the run installs each prelude after the Engine
    /// globals and before any block runs, so every block, and every library
    /// a block loads with `require`, can call what the preludes define.
    ///
    /// A prelude fails the run as
    /// [`RunErrorKind::Lua`](super::RunErrorKind::Lua) if it fails to load,
    /// or if it defines a global that another prelude, an Engine global, or
    /// a reserved name already holds.
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
    /// `prepare` copies the catalog, the roster, and the preludes onto
    /// `ctx` and binds the prompt's model roles. It returns the enriched
    /// context together with the report. It uses the context's filesystem
    /// as given, because the caller builds the run's whole filesystem,
    /// including its real directories and the declared store. The caller
    /// checks before `prepare` that each Plugin the prompt declares can
    /// serve, and merges that report into the one `prepare` returns.
    ///
    /// Every declared model role binds to the context's current model.
    /// `prepare` then checks each role's hard keywords (`thinking`,
    /// `no-thinking`) and context minimum against the model's descriptor.
    /// Each failed check is listed in [`Requirements::unmet_requirements`]
    /// with the required and actual values, and the role stays bound to
    /// the current model. Soft keywords record the author's intent only,
    /// so `prepare` skips them. When the context's current model is
    /// `None`, `prepare` skips the fill and the checks, and the model
    /// bindings stay empty.
    #[must_use]
    pub fn prepare(&self, prompt: &Prompt, ctx: RunContext) -> (RunContext, Requirements) {
        let mut ctx = ctx;
        let mut requirements = Requirements::default();
        ctx.tools = self.tools.clone();
        ctx.roster.clone_from(&self.roster);
        ctx.preludes.clone_from(&self.preludes);
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
            .field("roster", &self.roster)
            .field(
                "preludes",
                &self
                    .preludes
                    .iter()
                    .map(Prelude::plugin)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
