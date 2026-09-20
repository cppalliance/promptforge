//! Registry tests: duplicate-id rejection, exact lookup, and the
//! punctuation-twin normalization collision.

use std::sync::Arc;

use promptforge_api_types::capabilities::CapabilityId;

use super::{CapabilityRegistry, RegistryErrorKind};
use crate::{Capability, CapabilityError, Contribution, RunServices};

/// A minimal capability carrying a fixed id and description.
struct Stub {
    id: CapabilityId,
    description: String,
}

impl Capability for Stub {
    fn id(&self) -> &CapabilityId {
        &self.id
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        let _ = services;
        Ok(Contribution::default())
    }
}

/// Parses a test capability id.
fn capability_id(id: &str) -> CapabilityId {
    CapabilityId::parse(id).expect("test ids are valid capability ids")
}

/// Builds a stub capability with a fixed id and description.
fn stub(id: &str, description: &str) -> Arc<dyn Capability> {
    Arc::new(Stub {
        id: capability_id(id),
        description: description.to_owned(),
    })
}

#[test]
fn registering_a_second_capability_under_the_same_id_is_rejected() {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(stub("promptforge/web", "Web tools."))
        .expect("the first registration succeeds");
    let error = registry
        .register(stub("promptforge/web", "Other web tools."))
        .expect_err("a duplicate id is rejected");
    assert_eq!(error.kind(), RegistryErrorKind::DuplicateId);
    assert_eq!(error.id(), &capability_id("promptforge/web"));
    // The first registration survives the rejected duplicate.
    assert_eq!(
        registry
            .get(&capability_id("promptforge/web"))
            .map(|capability| capability.description()),
        Some("Web tools.")
    );
}

#[test]
fn a_registered_capability_resolves_by_exact_id_lookup() {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(stub("promptforge/web", "Web tools."))
        .expect("the first registration succeeds");
    registry
        .register(stub("org.rustalliance/core", "Core tools."))
        .expect("a distinct id registers");
    let found = registry
        .get(&capability_id("org.rustalliance/core"))
        .expect("the registered id resolves");
    assert_eq!(found.description(), "Core tools.");
    assert!(registry.get(&capability_id("promptforge/fs")).is_none());
}

#[test]
fn a_punctuation_twin_of_a_registered_id_is_rejected() {
    for twin in ["acme/web_search", "acme/web.search"] {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(stub("acme/web-search", "Web tools."))
            .expect("the first registration succeeds");
        let error = registry
            .register(stub(twin, "Other web tools."))
            .expect_err("a punctuation twin is rejected");
        assert_eq!(error.kind(), RegistryErrorKind::NormalizationCollision);
        assert_eq!(
            error.collides_with(),
            Some(&capability_id("acme/web-search"))
        );
        let message = error.to_string();
        assert!(
            message.contains("acme/web-search"),
            "the message names the registered id: {message}"
        );
        assert!(
            message.contains(twin),
            "the message names the rejected id: {message}"
        );
        // The rejected twin is not registered; the original survives.
        assert!(registry.get(&capability_id(twin)).is_none());
        assert!(registry.get(&capability_id("acme/web-search")).is_some());
    }
}

#[test]
fn punctuation_distinct_non_twins_register() {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(stub("acme/web-search", "Web tools."))
        .expect("the first registration succeeds");
    registry
        .register(stub("acme/web-search-extra", "Extra web tools."))
        .expect("a punctuation-distinct non-twin registers");
    assert!(
        registry
            .get(&capability_id("acme/web-search-extra"))
            .is_some()
    );
}
