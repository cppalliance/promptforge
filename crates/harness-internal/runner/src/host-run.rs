//! One run's snapshot of the Host's Plugins: [`HostRunContext`], which
//! holds what each installed Plugin looks like to the run and performs
//! the run's tool calls.
//!
//! The snapshot reads each usable Plugin's tool list once, after the wait
//! for every Plugin to be ready, so the run's catalog and the tools its
//! calls may reach stay fixed for the run. A tool call goes to the Plugin
//! its id's first segment names.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use promptforge::effect::ToolCallOrigin;
use promptforge::plugins::Prelude;
use promptforge::prompt::ToolSlot;
use promptforge::tools::{
    ToolCatalog, ToolDescriptor, ToolError, ToolErrorKind, ToolId, ToolOutput,
};
use promptforge::vfs::Access;
use promptforge::{MissingService, Prompt, Requirements, UnavailablePlugin};
use promptforge_plugin::{HostServices, Package, Plugin, PluginId, ServiceId, ToolContext};
use serde_json::Value;

use crate::performers::{BoxFuture, ToolPerformer};

/// One run's frozen snapshot of every installed Plugin, beside the run's
/// own services, which every call it performs is lent.
pub(crate) struct HostRunContext {
    services: HostServices,
    plugins: BTreeMap<PluginId, RunPlugin>,
}

/// What one installed Plugin looks like to a run.
enum RunPlugin {
    /// It can serve; `tools` is its tool list after validation.
    Usable {
        package: Package,
        plugin: Arc<dyn Plugin>,
        tools: Vec<ToolDescriptor>,
    },
    /// Its `construct` failed, or its `ready` did before this run, for
    /// this reason.
    Unavailable(String),
    /// The run lacks these services from its package's needs.
    NeedsUnmet(Vec<ServiceId>),
}

impl HostRunContext {
    /// Takes the snapshot of the `installed` Plugins, in install order,
    /// with `services` as the run's own.
    ///
    /// A built Plugin named in `failures` is unavailable with the reason
    /// it maps to. Any other built Plugin is usable when `services`
    /// provide every id its package needs; only then is its tool list read
    /// and validated.
    pub(super) fn snapshot<'a>(
        installed: impl Iterator<Item = (&'a PluginId, Package, &'a Result<Arc<dyn Plugin>, String>)>,
        services: HostServices,
        mut failures: BTreeMap<PluginId, String>,
    ) -> HostRunContext {
        let mut seen = BTreeSet::new();
        let plugins = installed
            .map(|(name, package, built)| {
                let state = match (built, failures.remove(name)) {
                    (Err(reason), _) => RunPlugin::Unavailable(reason.clone()),
                    (Ok(_), Some(reason)) => RunPlugin::Unavailable(reason),
                    (Ok(plugin), None) => {
                        let unmet: Vec<ServiceId> = package
                            .needs
                            .iter()
                            .copied()
                            .filter(|service| !services.provides(service))
                            .collect();
                        if unmet.is_empty() {
                            RunPlugin::Usable {
                                package,
                                plugin: Arc::clone(plugin),
                                tools: validated(name, plugin.tools(), &mut seen),
                            }
                        } else {
                            RunPlugin::NeedsUnmet(unmet)
                        }
                    }
                };
                (name.clone(), state)
            })
            .collect();
        HostRunContext { services, plugins }
    }

    /// Every usable Plugin's tools, declared by the prompt or not.
    pub(super) fn catalog(&self) -> ToolCatalog {
        let tools: Vec<ToolDescriptor> = self
            .plugins
            .values()
            .filter_map(|plugin| match plugin {
                RunPlugin::Usable { tools, .. } => Some(tools),
                RunPlugin::Unavailable(_) | RunPlugin::NeedsUnmet(_) => None,
            })
            .flatten()
            .cloned()
            .collect();
        ToolCatalog::new(&tools).unwrap_or_else(|error| {
            // Every tool passed containment and uniqueness validation one
            // at a time, so this build cannot fail.
            tracing::warn!(%error, "the run's catalog failed after per-tool validation");
            ToolCatalog::default()
        })
    }

    /// The preludes of the usable Plugins `prompt` declares, in
    /// declaration order, each under the Plugin's installed name.
    pub(super) fn preludes(&self, prompt: &Prompt) -> Vec<Prelude> {
        prompt
            .frontmatter()
            .plugins()
            .iter()
            .filter_map(|name| match self.plugins.get(name) {
                Some(RunPlugin::Usable { package, .. }) => package
                    .prelude
                    .map(|source| Prelude::new(name.clone(), source)),
                _ => None,
            })
            .collect()
    }

    /// What the snapshot cannot meet for `prompt`, for each Plugin it
    /// declares or names in a tool slot: one not installed, one that
    /// failed to build, and one whose needs the run lacks.
    pub(super) fn requirements(&self, prompt: &Prompt) -> Requirements {
        let frontmatter = prompt.frontmatter();
        let slotted = frontmatter.tools().iter().filter_map(|(_alias, slot)| {
            let ToolSlot::Exact(tool) = slot else {
                return None;
            };
            Some(tool.plugin())
        });
        let mut named: Vec<PluginId> = Vec::new();
        for name in frontmatter.plugins().iter().cloned().chain(slotted) {
            if !named.contains(&name) {
                named.push(name);
            }
        }
        let mut requirements = Requirements::default();
        for name in named {
            match self.plugins.get(&name) {
                None => requirements.missing_required.push(name),
                Some(RunPlugin::Unavailable(reason)) => requirements
                    .unavailable
                    .push(UnavailablePlugin::new(name, reason.clone())),
                Some(RunPlugin::NeedsUnmet(unmet)) => {
                    for service in unmet {
                        tracing::warn!(
                            plugin = %name,
                            %service,
                            "the prompt needs a Plugin whose service this run lacks"
                        );
                        requirements
                            .missing_services
                            .push(MissingService::new(name.clone(), service.to_string()));
                    }
                }
                Some(RunPlugin::Usable { .. }) => {}
            }
        }
        requirements
    }

    /// The usable Plugin whose snapshot offers `tool`, and that tool's
    /// descriptor.
    fn offering(&self, tool: &ToolId) -> Option<(&Arc<dyn Plugin>, &ToolDescriptor)> {
        match self.plugins.get(&tool.plugin())? {
            RunPlugin::Usable { plugin, tools, .. } => tools
                .iter()
                .find(|descriptor| descriptor.id == *tool)
                .map(|descriptor| (plugin, descriptor)),
            RunPlugin::Unavailable(_) | RunPlugin::NeedsUnmet(_) => None,
        }
    }
}

impl ToolPerformer for HostRunContext {
    fn call(
        &self,
        tool: ToolId,
        alias: String,
        access: Arc<Access>,
        origin: ToolCallOrigin,
        args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let plugin = self.offering(&tool).map(|(plugin, _)| Arc::clone(plugin));
        let services = self.services.clone();
        Box::pin(async move {
            let Some(plugin) = plugin else {
                tracing::error!(
                    tool = %tool,
                    alias = %alias,
                    "a ToolCall names a tool no usable Plugin of the run offers"
                );
                return Err(ToolError::message(format!(
                    "tool `{alias}` ({tool}) is not offered by any usable Plugin of this run"
                ))
                .with_kind(ToolErrorKind::Other));
            };
            let cx = ToolContext::new(&tool, &access, &origin, &services);
            plugin.call(cx, args).await
        })
    }

    fn survives_stop(&self, tool: &ToolId) -> bool {
        self.offering(tool)
            .is_some_and(|(_, descriptor)| descriptor.survives_stop)
    }
}

impl fmt::Debug for HostRunContext {
    /// Shows the run's services and each Plugin's name and state.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plugins: Vec<(&PluginId, &str)> = self
            .plugins
            .iter()
            .map(|(name, plugin)| {
                let state = match plugin {
                    RunPlugin::Usable { .. } => "usable",
                    RunPlugin::Unavailable(_) => "unavailable",
                    RunPlugin::NeedsUnmet(_) => "needs unmet",
                };
                (name, state)
            })
            .collect();
        f.debug_struct("HostRunContext")
            .field("services", &self.services)
            .field("plugins", &plugins)
            .finish()
    }
}

/// The tools of the Plugin installed as `plugin` that pass validation: a
/// tool must sit under the Plugin's name and must not repeat an id in
/// `seen`. A tool that fails is logged and dropped, so it costs only
/// itself.
fn validated(
    plugin: &PluginId,
    tools: Vec<ToolDescriptor>,
    seen: &mut BTreeSet<ToolId>,
) -> Vec<ToolDescriptor> {
    tools
        .into_iter()
        .filter(|tool| {
            if !plugin.contains(&tool.id) {
                tracing::warn!(
                    plugin = %plugin,
                    tool = %tool.id,
                    "a Plugin's tool id escapes its name; dropped"
                );
                return false;
            }
            if seen.contains(&tool.id) {
                tracing::warn!(
                    plugin = %plugin,
                    tool = %tool.id,
                    "a Plugin's tool id repeats an earlier tool; dropped"
                );
                return false;
            }
            seen.insert(tool.id.clone());
            true
        })
        .collect()
}
