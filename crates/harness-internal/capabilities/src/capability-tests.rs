//! Tests for the capability activation contract.

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;

use super::{Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices};

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
fn a_capability_declares_no_conflicts_by_default() {
    let capability = StubCapability::web();
    assert!(capability.conflicts().is_empty());
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
    let services = RunServices::new(
        promptforge::vfs::VfsRef::builder().build(),
        CancelHandle::new(),
    );
    let contribution = capability
        .create(&services)
        .expect("activation succeeds on a live run");
    assert!(contribution.tools.is_empty());

    let cancel = CancelHandle::new();
    cancel.cancel();
    let services = RunServices::new(promptforge::vfs::VfsRef::builder().build(), cancel);
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
        "the cause stays behind Error::source, out of the model-readable message"
    );

    let cancelled = CapabilityError::message("stopped").with_kind(CapabilityErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());
    assert!(!CapabilityError::message("x").is_cancelled());
}
