//! Plugin activation: the harness-side step that turns a prompt's
//! declared Plugins into the run's [`ToolCatalog`] and the
//! implementations behind it.
//!
//! Before a run is prepared, the Harness resolves the prompt's declarations
//! against its [`PluginRegistry`], activates each present Plugin with the
//! run's [`RunServices`], and assembles the contributions into three things: the
//! [`ToolCatalog`] of descriptors [`Environment::prepare`] fills slots
//! against, the [`Prelude`]s every section VM installs, and the
//! [`ToolTable`] of implementations the Harness's tool performer resolves a
//! `ToolCall` effect's id in. The Engine sees only the first two.
//!
//! [`Environment::prepare`]: promptforge::Environment::prepare

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use promptforge::Prompt;
use promptforge::plugins::{PluginId, Prelude};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{MissingService, Requirements};

use crate::plugin::{Contribution, RunServices};
use crate::registry::PluginRegistry;
use crate::service::ServiceId;
use crate::tool::Tool;

#[cfg(test)]
#[path = "activation-tests.rs"]
mod tests;

/// The implementations behind a run's catalog, keyed by stable identity.
///
/// Held by the Harness: a `ToolCall` effect from the Engine names a
/// [`ToolId`], and the Harness's tool performer resolves it here.
#[derive(Clone, Default)]
pub struct ToolTable {
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
}

impl ToolTable {
    /// Builds an empty table.
    #[must_use]
    pub fn new() -> ToolTable {
        ToolTable::default()
    }

    /// Adds `tool` under its own identity; a repeated identity keeps the
    /// first implementation.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.entry(tool.id()).or_insert(tool);
    }

    /// Returns the implementation registered under `id`.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<Arc<dyn Tool>> {
        self.tools.get(id).map(Arc::clone)
    }

    /// Returns whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl fmt::Debug for ToolTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolTable")
            .field("ids", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What activating a prompt's declared Plugins produced.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Activation {
    /// The activated Plugins' contributed tools as descriptors, in
    /// declaration order: what the Harness hands to
    /// [`Environment::tools`](promptforge::Environment::tools).
    pub catalog: ToolCatalog,
    /// The implementations behind the catalog: what the Harness's tool
    /// performer resolves against.
    pub tools: ToolTable,
    /// The activated Plugins' preludes, in declaration order: what
    /// the Harness hands to
    /// [`Environment::preludes`](promptforge::Environment::preludes). A
    /// Plugin that does not activate contributes none.
    pub preludes: Vec<Prelude>,
    /// What activation could not satisfy: the declared Plugins that
    /// are absent or failed to activate, and the declared Plugins that
    /// need a run service this Host does not provide. Merged into the
    /// prepare report through [`Requirements::merge`] so one refusal names
    /// every gap.
    pub requirements: Requirements,
}

/// Resolves and activates the Plugins `prompt` declares against
/// `registry`, assembling the run's catalog and implementation table.
///
/// Every declared Plugin is required. Declared Plugins resolve against
/// the registry in declaration order, and an absent one lands in
/// [`Requirements::missing_required`]. Each present Plugin's
/// [`needs`](crate::Plugin::needs) are checked against
/// [`RunServices::provides`] before any Plugin code runs, so a provider
/// of another type than the id names counts as missing: a Plugin that
/// needs a service `services` does not provide is not activated and lands
/// in [`Requirements::missing_services`] under the service's id, once per
/// missing service. Each remaining Plugin is activated with `services`
/// (the run's cancellation handle and the services it has); an
/// activation failure is logged, the Plugin contributes nothing, and it
/// lands in [`Requirements::missing_required`], since the run cannot have
/// what the prompt declared.
///
/// The activated contributions are assembled into the catalog in
/// declaration order, with tool prefix-containment enforced at assembly: a
/// contributed tool whose id escapes its Plugin's id, repeats an
/// earlier contribution, or has a transport-illegal wire name is
/// rejected - logged and never admitted. Each activated Plugin's
/// prelude, when its contribution has one, lands in
/// [`Activation::preludes`] in the same declaration order.
#[must_use]
pub fn activate(
    registry: Option<&PluginRegistry>,
    prompt: &Prompt,
    services: &RunServices,
) -> Activation {
    let mut requirements = Requirements::default();
    let mut activated: Vec<(PluginId, Contribution)> = Vec::new();
    for id in prompt.frontmatter().plugins() {
        let Some(plugin) = registry.and_then(|registry| registry.get(id)) else {
            requirements.missing_required.push(id.clone());
            continue;
        };
        let unprovided: Vec<ServiceId> = plugin
            .needs()
            .iter()
            .copied()
            .filter(|service| !services.provides(service))
            .collect();
        if !unprovided.is_empty() {
            for service in unprovided {
                tracing::warn!(
                    plugin = %id,
                    %service,
                    "declared Plugin needs a service this host does not provide; it does not activate"
                );
                requirements
                    .missing_services
                    .push(MissingService::new(id.clone(), service.to_string()));
            }
            continue;
        }
        match plugin.create(services) {
            Ok(contribution) => {
                tracing::info!(plugin = %id, "Plugin activated");
                activated.push((id.clone(), contribution));
            }
            Err(error) => {
                tracing::warn!(
                    plugin = %id,
                    %error,
                    "Plugin activation failed; it contributes nothing to the run"
                );
                // A declared Plugin that cannot activate leaves the run
                // without something the prompt declared: report it like an
                // absent one so the run fails until satisfied.
                requirements.missing_required.push(id.clone());
            }
        }
    }
    let (catalog, tools) = assemble(&activated);
    let preludes = activated
        .into_iter()
        .filter_map(|(id, contribution)| {
            contribution.prelude.map(|source| Prelude::new(id, source))
        })
        .collect();
    Activation {
        catalog,
        tools,
        preludes,
        requirements,
    }
}

/// Assembles the run's catalog and implementation table from the activated
/// Plugins' contributions in declaration order.
///
/// Containment is total and enforced here: every contributed tool's id must
/// sit under its contributing Plugin's name (`web/fetch` for a `web`
/// Plugin). A violating tool - like a repeated id or a transport-illegal
/// wire name - is rejected at assembly: logged and never admitted.
fn assemble(activated: &[(PluginId, Contribution)]) -> (ToolCatalog, ToolTable) {
    let mut descriptors: Vec<ToolDescriptor> = Vec::new();
    let mut table = ToolTable::new();
    let mut seen = std::collections::BTreeSet::new();
    for (plugin, contribution) in activated {
        for tool in &contribution.tools {
            let id = tool.id();
            if !plugin.contains(&id) {
                tracing::warn!(
                    plugin = %plugin,
                    tool = %id,
                    "contributed tool id escapes its Plugin's id; rejected at assembly"
                );
                continue;
            }
            if !seen.insert(id.clone()) {
                tracing::warn!(
                    plugin = %plugin,
                    tool = %id,
                    "contributed tool id repeats an earlier contribution; rejected at assembly"
                );
                continue;
            }
            let descriptor = tool.descriptor();
            // The catalog is the transport boundary: validate the wire name
            // per tool so one bad tool costs only itself.
            if let Err(error) = ToolCatalog::new(std::slice::from_ref(&descriptor)) {
                tracing::warn!(
                    plugin = %plugin,
                    tool = %id,
                    %error,
                    "contributed tool failed catalog validation; rejected at assembly"
                );
                continue;
            }
            descriptors.push(descriptor);
            table.insert(Arc::clone(tool));
        }
    }
    let catalog = match ToolCatalog::new(&descriptors) {
        Ok(catalog) => catalog,
        Err(error) => {
            // Every accepted descriptor passed containment, uniqueness, and
            // wire-name validation above, so this build cannot fail; the
            // arm is defensive.
            tracing::warn!(%error, "catalog assembly failed after per-tool validation");
            ToolCatalog::default()
        }
    };
    (catalog, table)
}
