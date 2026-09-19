//! The run's journaled bindings: [`ModelBindings`] and [`ToolBindings`].

use std::collections::BTreeMap;

use promptforge_api_types::tools::ToolDescriptor;

use crate::model::{ModelDescriptor, ModelId};
use crate::tools::ToolId;

/// The run's model satisfaction: which concrete model each declared role
/// is bound to, and the descriptors of every model this run may use.
///
/// Written by the fill function at
/// [`prepare`](super::Environment::prepare); v1's fill is deliberately
/// trivial - every declared role binds to the context's current model.
/// The structure is general from day one (a table of models and a map of
/// roles) so multi-model satisfaction arrives as a smarter fill function,
/// never a structural change. Handles resolve label -> id -> descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelBindings {
    /// The decision, journaled: role label to the bound model's identity.
    roles: BTreeMap<String, ModelId>,
    /// What this run may use: identity to descriptor.
    models: BTreeMap<ModelId, ModelDescriptor>,
}

impl ModelBindings {
    /// Binds the role `label` to `model`, recording the descriptor under
    /// its identity. The fill function's only writer.
    pub(crate) fn bind(&mut self, label: &str, model: ModelDescriptor) {
        self.roles.insert(label.to_owned(), model.id().clone());
        self.models.entry(model.id().clone()).or_insert(model);
    }

    /// Returns the identity bound to the role `label`, when it was filled.
    #[must_use]
    pub fn role_id(&self, label: &str) -> Option<&ModelId> {
        self.roles.get(label)
    }

    /// Resolves a role label all the way to its descriptor:
    /// label -> id -> descriptor.
    #[must_use]
    pub fn resolve(&self, label: &str) -> Option<&ModelDescriptor> {
        self.roles.get(label).and_then(|id| self.models.get(id))
    }

    /// Returns the descriptor bound under `id`, when this run may use it.
    #[must_use]
    pub fn model(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.get(id)
    }

    /// Returns the number of bound roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns whether no roles are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

/// The run's tool bindings: which tool each declared alias is bound to,
/// and the descriptors of every tool this run may call.
///
/// Written by [`prepare`](super::Environment::prepare)'s slot fill: exact
/// slots fill by identity against the host-supplied catalog, and every
/// fill is journaled here so hosts and evals see what each alias resolved
/// to. The bindings carry descriptors, never implementations: the engine
/// advertises and calls a tool by its data, and the host resolves the id
/// a `ToolCall` effect names. The model only ever sees the prompt-local
/// alias, never the global path. Handles resolve alias -> id -> descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolBindings {
    /// The decision, journaled: prompt-local alias to the bound tool's
    /// identity.
    aliases: BTreeMap<String, ToolId>,
    /// What this run may call: identity to descriptor.
    tools: BTreeMap<ToolId, ToolDescriptor>,
}

impl ToolBindings {
    /// Binds the prompt-local `alias` to the tool `descriptor` describes,
    /// recording the descriptor under its identity. The slot fill's only
    /// writer.
    pub(crate) fn bind(&mut self, alias: &str, descriptor: ToolDescriptor) {
        self.aliases.insert(alias.to_owned(), descriptor.id.clone());
        self.tools
            .entry(descriptor.id.clone())
            .or_insert(descriptor);
    }

    /// Returns the identity bound to `alias`, when the slot was filled.
    #[must_use]
    pub fn alias_id(&self, alias: &str) -> Option<&ToolId> {
        self.aliases.get(alias)
    }

    /// Resolves a prompt-local alias all the way to its descriptor:
    /// alias -> id -> descriptor.
    #[must_use]
    pub fn resolve(&self, alias: &str) -> Option<&ToolDescriptor> {
        self.aliases.get(alias).and_then(|id| self.tools.get(id))
    }

    /// Returns the descriptor bound under `id`, when this run may call it.
    #[must_use]
    pub fn tool(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.get(id)
    }

    /// Returns the number of bound aliases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.aliases.len()
    }

    /// Returns whether no aliases are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::model::ThinkingMode;

    fn descriptor(name: &str) -> ModelDescriptor {
        ModelDescriptor::new(
            ModelId::gateway(name).expect("the test id is valid"),
            "A test model",
            NonZeroU32::new(32_000).expect("the context window is non-zero"),
            ThinkingMode::Switchable,
        )
    }

    #[test]
    fn an_empty_binding_set_resolves_nothing() {
        let bindings = ModelBindings::default();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert!(bindings.role_id("analyst").is_none());
        assert!(bindings.resolve("analyst").is_none());
    }

    #[test]
    fn two_roles_bound_to_one_model_share_one_descriptor_entry() {
        // v1's trivial fill: every role binds the same model, and the
        // descriptor table holds it once - the seam a smarter fill grows
        // into is visible in the shape, not the content.
        let model = descriptor("current");
        let mut bindings = ModelBindings::default();
        bindings.bind("analyst", model.clone());
        bindings.bind("triage", model.clone());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.role_id("analyst"), Some(model.id()));
        assert_eq!(bindings.role_id("triage"), Some(model.id()));
        assert_eq!(bindings.resolve("analyst"), Some(&model));
        assert_eq!(bindings.resolve("triage"), Some(&model));
        assert_eq!(bindings.model(model.id()), Some(&model));
        assert!(
            bindings
                .model(&ModelId::gateway("other").expect("valid"))
                .is_none()
        );
    }

    /// A fixture descriptor under `id`: a static wire name and an empty
    /// schema.
    fn fixture(id: &ToolId) -> ToolDescriptor {
        ToolDescriptor::new(
            id.clone(),
            "fixture",
            "A fixture tool.",
            serde_json::json!({"type": "object", "properties": {}}),
        )
    }

    #[test]
    fn an_empty_tool_binding_set_resolves_nothing() {
        let bindings = ToolBindings::default();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert!(bindings.alias_id("fetch").is_none());
        assert!(bindings.resolve("fetch").is_none());
    }

    #[test]
    fn two_aliases_bound_to_one_tool_share_one_tool_entry() {
        // Two slots may fill to the same tool; the tool table holds it
        // once and both aliases resolve alias -> id -> tool.
        let id = ToolId::parse("promptforge/web/fetch").expect("the test id is valid");
        let tool = fixture(&id);
        let mut bindings = ToolBindings::default();
        bindings.bind("fetch", tool.clone());
        bindings.bind("getter", tool);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.alias_id("fetch"), Some(&id));
        assert_eq!(bindings.alias_id("getter"), Some(&id));
        assert_eq!(
            bindings.resolve("fetch").map(|tool| tool.id.clone()),
            Some(id.clone())
        );
        assert_eq!(
            bindings.resolve("getter").map(|tool| tool.id.clone()),
            Some(id.clone())
        );
        assert!(bindings.tool(&id).is_some());
        assert!(
            bindings
                .tool(&ToolId::parse("promptforge/web/search").expect("valid"))
                .is_none()
        );
    }
}
