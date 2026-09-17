//! Registry tests: duplicate-id rejection, exact lookup, and the
//! registration-time near-duplicate description lint.

use std::io;
use std::sync::{Arc, Mutex};

use promptforge_api_types::capabilities::{
    Capability, CapabilityError, CapabilityId, Contribution, RunServices,
};

use super::{CapabilityRegistry, RegistryErrorKind};

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

/// A shared buffer a fmt subscriber writes lint warnings into.
#[derive(Clone, Default)]
struct Buffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("the buffer lock is not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `f` under a fmt subscriber writing into a shared buffer and
/// returns everything the subscriber captured.
fn captured_warnings(f: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buffer
        .bytes
        .lock()
        .expect("the buffer lock is not poisoned");
    String::from_utf8_lossy(&bytes).into_owned()
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
fn registering_a_near_duplicate_description_fires_the_lint() {
    let warnings = captured_warnings(|| {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(stub("promptforge/web", "Fetch and render a web page."))
            .expect("the first registration succeeds");
        registry
            .register(stub("org.rustalliance/web", "Fetch and render a web page."))
            .expect("a near-duplicate description warns without rejecting");
    });
    assert!(
        warnings.contains("near-duplicates"),
        "the lint fired: {warnings}"
    );
    assert!(
        warnings.contains("promptforge/web"),
        "the warning names the first capability: {warnings}"
    );
    assert!(
        warnings.contains("org.rustalliance/web"),
        "the warning names the second capability: {warnings}"
    );
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

#[test]
fn distinct_descriptions_do_not_fire_the_lint() {
    let warnings = captured_warnings(|| {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(stub("promptforge/web", "Fetch and render a web page."))
            .expect("the first registration succeeds");
        registry
            .register(stub(
                "promptforge/fs",
                "Read and write files in the run store.",
            ))
            .expect("a distinct description registers");
    });
    assert!(
        !warnings.contains("near-duplicates"),
        "no lint fired: {warnings}"
    );
}
