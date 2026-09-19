//! Tests for the capability activation contract.

use std::sync::Arc;

use super::{
    Capability, CapabilityError, CapabilityErrorKind, CapabilityId, CapabilityIdErrorKind,
    Contribution, RunServices,
};
use crate::cancel::sync::CancelHandle;
use crate::tools::ToolId;

/// A minimal in-process capability: a static id, no contributed tools, and
/// a `create` that refuses a cancelled run so tests can observe the
/// services it was handed.
struct StubCapability {
    id: CapabilityId,
    description: String,
}

impl StubCapability {
    fn web() -> StubCapability {
        StubCapability {
            id: CapabilityId::parse("promptforge/web").expect("a static valid id"),
            description: "A stub capability that contributes nothing.".to_owned(),
        }
    }
}

impl Capability for StubCapability {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        if services.cancel.is_cancelled() {
            return Err(
                CapabilityError::message("activation cancelled before create")
                    .with_kind(CapabilityErrorKind::Cancelled),
            );
        }
        Ok(Contribution::default())
    }
}

/// Compile-time proof that a capability can be shared across tasks and
/// threads behind a trait object: the registry stores `Arc<dyn Capability>`.
const fn _assert_capability_trait_object_is_shareable() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Arc<dyn Capability>>();
}

#[test]
fn capability_id_requires_exactly_two_segments() {
    let id = CapabilityId::parse("promptforge/web").expect("two segments parse");
    assert_eq!(id.to_string(), "promptforge/web");

    let three = CapabilityId::parse("promptforge/web/fetch")
        .expect_err("three segments are a tool id, not a capability id");
    assert_eq!(three.kind(), CapabilityIdErrorKind::SegmentCount);

    let one = CapabilityId::parse("promptforge").expect_err("one segment names nothing");
    assert_eq!(one.kind(), CapabilityIdErrorKind::SegmentCount);
}

#[test]
fn capability_id_rejects_empty_and_control_segments() {
    let empty = CapabilityId::parse("promptforge/").expect_err("an empty segment is rejected");
    assert_eq!(empty.kind(), CapabilityIdErrorKind::Empty);

    let upper = CapabilityId::parse("Promptforge/web")
        .expect_err("comparison is case-sensitive: uppercase is outside the charset");
    assert_eq!(upper.kind(), CapabilityIdErrorKind::Control);

    let versioned = CapabilityId::parse("promptforge/web@2")
        .expect_err("v1 is unversioned: '@' is a parse error");
    assert_eq!(versioned.kind(), CapabilityIdErrorKind::Control);
}

#[test]
fn capability_id_exposes_namespace_and_pack() {
    let id = CapabilityId::parse("org.rustalliance/core").expect("a reverse-DNS namespace parses");
    assert_eq!(id.namespace(), "org.rustalliance");
    assert_eq!(id.pack(), "core");
}

#[test]
fn capability_id_contains_exactly_the_tools_under_it() {
    let web = CapabilityId::parse("promptforge/web").expect("a static valid id");
    let fetch = ToolId::parse("promptforge/web/fetch").expect("a static valid id");
    assert!(web.contains(&fetch));
    let stray = ToolId::parse("promptforge/other/fetch").expect("a static valid id");
    assert!(!web.contains(&stray));
    // Containment is by identity, not by prefix text: a pack whose name
    // merely extends this one is not contained.
    let extended = ToolId::parse("promptforge/web2/fetch").expect("a static valid id");
    assert!(!web.contains(&extended));
}

#[test]
fn a_capability_declares_no_conflicts_by_default() {
    let capability = StubCapability::web();
    assert!(capability.conflicts().is_empty());
}

#[test]
fn capability_id_serializes_as_its_string_form() {
    let id = CapabilityId::parse("promptforge/web").expect("a static valid id");
    let json = serde_json::to_string(&id).expect("serializes");
    assert_eq!(json, "\"promptforge/web\"");
    let back: CapabilityId = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, id);
    assert!(
        serde_json::from_str::<CapabilityId>("\"promptforge/web/fetch\"").is_err(),
        "a 3-segment string is a tool id, never a capability id"
    );
}

#[test]
fn a_capability_is_object_safe_and_exposes_its_identity() {
    let capability: Arc<dyn Capability> = Arc::new(StubCapability::web());
    assert_eq!(capability.id().to_string(), "promptforge/web");
    assert!(!capability.description().is_empty());
}

#[test]
fn a_default_contribution_has_no_tools() {
    let contribution = Contribution::default();
    assert!(contribution.tools.is_empty());
}

#[test]
fn create_receives_the_run_services() {
    let capability = StubCapability::web();
    let services = RunServices::new(shared_vfs::VfsRef::builder().build(), CancelHandle::new());
    let contribution = capability
        .create(&services)
        .expect("activation succeeds on a live run");
    assert!(contribution.tools.is_empty());

    let cancel = CancelHandle::new();
    cancel.cancel();
    let services = RunServices::new(shared_vfs::VfsRef::builder().build(), cancel);
    let error = capability
        .create(&services)
        .expect_err("a cancelled run fails activation");
    assert!(error.is_cancelled());
}

#[test]
fn capability_error_display_is_the_model_readable_message() {
    let error = CapabilityError::message("the fs capability needs a writable store");
    assert_eq!(
        error.to_string(),
        "the fs capability needs a writable store"
    );
    assert_eq!(error.kind(), CapabilityErrorKind::Other);
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn capability_error_classifies_and_hides_its_cause() {
    let io = std::io::Error::other("disk full");
    let error = CapabilityError::with_source("activation failed", io);
    assert_eq!(error.kind(), CapabilityErrorKind::Activation);
    assert_eq!(error.to_string(), "activation failed");
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause rides behind Error::source, out of the model-readable message"
    );

    let cancelled = CapabilityError::message("stopped").with_kind(CapabilityErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());
    assert!(!CapabilityError::message("x").is_cancelled());
}
