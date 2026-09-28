//! Tests for the capability activation contract.

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;

use super::{Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices, Service};
use crate::{InputBroker, InputError};

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
fn a_capability_needs_no_host_service_by_default() {
    let capability = StubCapability::web();
    assert!(capability.needs().is_empty());
}

#[test]
fn the_input_service_is_named_for_a_model_reader() {
    assert_eq!(Service::Input.description(), "an input broker");
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
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    let contribution = capability
        .create(&services)
        .expect("activation succeeds on a live run");
    assert!(contribution.tools.is_empty());

    let cancel = CancelHandle::new();
    cancel.cancel();
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), cancel);
    let error = capability
        .create(&services)
        .expect_err("a cancelled run fails activation");
    assert!(error.is_cancelled());
}

/// A broker whose operator always types the same text.
struct Scripted(&'static str);

#[async_trait::async_trait]
impl InputBroker for Scripted {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(self.0.to_owned())
    }
}

#[tokio::test]
async fn new_services_have_no_input_broker_and_with_input_supplies_one() {
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    assert!(
        services.input.is_none(),
        "a host that supplies no broker leaves the run without one"
    );
    assert!(
        format!("{services:?}").contains("input: false"),
        "Debug says whether a broker is present: {services:?}"
    );

    let services = services.with_input(Arc::new(Scripted("typed")));
    let broker = services
        .input
        .as_ref()
        .expect("with_input supplies the broker");
    assert_eq!(broker.wait().await.expect("the broker answers"), "typed");
    assert!(
        format!("{services:?}").contains("input: true"),
        "Debug says whether a broker is present: {services:?}"
    );
}

#[test]
fn the_input_service_is_provided_exactly_when_a_broker_is_present() {
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    assert!(!services.provides(Service::Input));
    let services = services.with_input(Arc::new(Scripted("typed")));
    assert!(services.provides(Service::Input));
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
