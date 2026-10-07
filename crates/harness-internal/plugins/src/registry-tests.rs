//! Registry tests: duplicate-id rejection, exact lookup, and the
//! punctuation-twin normalization collision.

use std::sync::Arc;

use promptforge::plugins::PluginId;

use super::{PluginRegistry, RegistryErrorKind};
use crate::{Contribution, Plugin, PluginError, RunServices};

/// A minimal Plugin with a fixed id and description.
struct Stub {
    id: PluginId,
    description: String,
}

impl Plugin for Stub {
    fn id(&self) -> &PluginId {
        &self.id
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn create(&self, services: &RunServices) -> Result<Contribution, PluginError> {
        let _ = services;
        Ok(Contribution::default())
    }
}

/// Parses a test Plugin id.
fn plugin_id(id: &str) -> PluginId {
    PluginId::parse(id).expect("test ids are valid Plugin ids")
}

/// Builds a stub Plugin with a fixed id and description.
fn stub(id: &str, description: &str) -> Arc<dyn Plugin> {
    Arc::new(Stub {
        id: plugin_id(id),
        description: description.to_owned(),
    })
}

#[test]
fn registering_a_second_plugin_under_the_same_id_is_rejected() {
    let mut registry = PluginRegistry::new();
    registry
        .register(stub("web", "Web tools."))
        .expect("the first registration succeeds");
    let error = registry
        .register(stub("web", "Other web tools."))
        .expect_err("a duplicate id is rejected");
    assert_eq!(error.kind(), RegistryErrorKind::DuplicateId);
    assert_eq!(error.id(), &plugin_id("web"));
    // The first registration survives the rejected duplicate.
    assert_eq!(
        registry
            .get(&plugin_id("web"))
            .map(|plugin| plugin.description()),
        Some("Web tools.")
    );
}

#[test]
fn a_registered_plugin_resolves_by_exact_id_lookup() {
    let mut registry = PluginRegistry::new();
    registry
        .register(stub("web", "Web tools."))
        .expect("the first registration succeeds");
    registry
        .register(stub("core", "Core tools."))
        .expect("a distinct id registers");
    let found = registry
        .get(&plugin_id("core"))
        .expect("the registered id resolves");
    assert_eq!(found.description(), "Core tools.");
    assert!(registry.get(&plugin_id("fs")).is_none());
}

#[test]
fn a_punctuation_twin_of_a_registered_id_is_rejected() {
    for twin in ["user_input", "user.input"] {
        let mut registry = PluginRegistry::new();
        registry
            .register(stub("user-input", "Ask the operator."))
            .expect("the first registration succeeds");
        let error = registry
            .register(stub(twin, "Ask someone else."))
            .expect_err("a punctuation twin is rejected");
        assert_eq!(error.kind(), RegistryErrorKind::NormalizationCollision);
        assert_eq!(error.collides_with(), Some(&plugin_id("user-input")));
        let message = error.to_string();
        assert!(
            message.contains("user-input"),
            "the message names the registered id: {message}"
        );
        assert!(
            message.contains(twin),
            "the message names the rejected id: {message}"
        );
        // The rejected twin is not registered; the original survives.
        assert!(registry.get(&plugin_id(twin)).is_none());
        assert!(registry.get(&plugin_id("user-input")).is_some());
    }
}

#[test]
fn punctuation_distinct_non_twins_register() {
    let mut registry = PluginRegistry::new();
    registry
        .register(stub("web-search", "Web tools."))
        .expect("the first registration succeeds");
    registry
        .register(stub("web-search-extra", "Extra web tools."))
        .expect("a punctuation-distinct non-twin registers");
    registry
        .register(stub("websearch", "Joined web tools."))
        .expect("dropping a separator is not a twin");
    assert!(registry.get(&plugin_id("web-search-extra")).is_some());
    assert!(registry.get(&plugin_id("websearch")).is_some());
}
