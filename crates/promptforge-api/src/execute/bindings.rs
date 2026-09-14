//! The run's model satisfaction: [`ModelBindings`].

use std::collections::BTreeMap;

use crate::model::{ModelDescriptor, ModelId};

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
}
