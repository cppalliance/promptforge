//! The run's journaled bindings: [`ModelBindings`] and [`ToolBindings`].

use std::collections::BTreeMap;

use promptforge_types::tools::ToolDescriptor;

use crate::model::{ModelDescriptor, ModelId};
use crate::tools::ToolId;

/// The models bound for a run: which model each declared role uses, and
/// the descriptor of every model the run may use.
///
/// [`prepare`](super::Environment::prepare) fills these bindings. It binds
/// every declared role to the run context's current model.
///
/// A lookup goes from a role label to a model identity, and from that
/// identity to the model's descriptor.
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
    pub(super) fn bind(&mut self, label: &str, model: ModelDescriptor) {
        self.roles.insert(label.to_owned(), model.id().clone());
        self.models.entry(model.id().clone()).or_insert(model);
    }

    /// Returns the identity of the model bound to the role `label`, or
    /// `None` when the role was not filled.
    #[must_use]
    pub fn role_id(&self, label: &str) -> Option<&ModelId> {
        self.roles.get(label)
    }

    /// Returns the descriptor of the model bound to the role `label`, or
    /// `None` when the role was not filled.
    #[must_use]
    pub fn resolve(&self, label: &str) -> Option<&ModelDescriptor> {
        self.roles.get(label).and_then(|id| self.models.get(id))
    }

    /// Returns the descriptor of the model with the identity `id`, or
    /// `None` when this run may not use that model.
    #[must_use]
    pub fn model(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.get(id)
    }

    /// Returns the number of bound roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns whether the set of bound roles is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

/// The tools bound for a run: which tool each declared alias names, and
/// the descriptor of every tool the run may call.
///
/// [`prepare`](super::Environment::prepare) fills these bindings from the
/// prompt's tool slots. It fills each exact slot by tool identity, from the
/// tool catalog the caller supplies. Every fill is recorded here, so the
/// caller and any evaluation of the run can see what each alias resolved
/// to.
///
/// The bindings hold tool descriptors. The Engine advertises and calls a
/// tool through its descriptor. The caller resolves the tool identity that
/// a `ToolCall` effect names. The model sees only the prompt-local alias.
///
/// A lookup goes from an alias to a tool identity, and from that identity
/// to the tool's descriptor.
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
    pub(super) fn bind(&mut self, alias: &str, descriptor: ToolDescriptor) {
        self.aliases.insert(alias.to_owned(), descriptor.id.clone());
        self.tools
            .entry(descriptor.id.clone())
            .or_insert(descriptor);
    }

    /// Returns the identity of the tool bound to the prompt-local `alias`,
    /// or `None` when its slot was not filled.
    #[must_use]
    pub fn alias_id(&self, alias: &str) -> Option<&ToolId> {
        self.aliases.get(alias)
    }

    /// Returns the descriptor of the tool bound to the prompt-local
    /// `alias`, or `None` when its slot was not filled.
    #[must_use]
    pub fn resolve(&self, alias: &str) -> Option<&ToolDescriptor> {
        self.aliases.get(alias).and_then(|id| self.tools.get(id))
    }

    /// Returns the descriptor of the tool with the identity `id`, or `None`
    /// when this run may not call that tool.
    #[must_use]
    pub fn tool(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.get(id)
    }

    /// Returns the number of bound aliases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.aliases.len()
    }

    /// Returns whether the set of bound aliases is empty.
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
        // The trivial fill binds every role to the same model, and the
        // descriptor table holds it once.
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
