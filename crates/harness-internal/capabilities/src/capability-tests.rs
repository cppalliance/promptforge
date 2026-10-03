//! Tests for the capability activation contract.

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;

use super::{Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices};
use crate::{HostServices, INPUT_BROKER, InputBroker, InputError, ServiceError, ServiceKey};

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
fn a_capability_is_object_safe_and_exposes_its_identity() {
    let capability: Arc<dyn Capability> = Arc::new(StubCapability::web());
    assert_eq!(capability.id().to_string(), "promptforge/web");
    assert!(!capability.description().is_empty());
}

#[test]
fn a_default_contribution_has_no_tools_and_no_prelude() {
    let contribution = Contribution::default();
    assert!(contribution.tools.is_empty());
    assert!(contribution.prelude.is_none());
}

#[test]
fn contribution_debug_says_whether_a_prelude_is_present_without_showing_it() {
    let absent = Contribution::default();
    assert!(
        format!("{absent:?}").contains("prelude: false"),
        "Debug says no prelude is present: {absent:?}"
    );
    let present = Contribution {
        tools: Vec::new(),
        prelude: Some("secret_table = {}".to_owned()),
    };
    let shown = format!("{present:?}");
    assert!(
        shown.contains("prelude: true"),
        "Debug says a prelude is present: {shown}"
    );
    assert!(
        !shown.contains("secret_table"),
        "Debug leaves the prelude's source out: {shown}"
    );
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

/// Host services holding `broker` under [`INPUT_BROKER`].
fn host_with_broker(broker: Scripted) -> HostServices {
    let mut host = HostServices::new();
    let broker: Arc<dyn InputBroker> = Arc::new(broker);
    host.provide(&INPUT_BROKER, broker)
        .expect("an empty map takes the broker");
    host
}

#[tokio::test]
async fn new_services_have_no_input_broker_and_the_hosts_services_supply_one() {
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    assert!(
        services.get(&INPUT_BROKER).is_none(),
        "a host that supplies no broker leaves the run without one"
    );
    assert!(
        !format!("{services:?}").contains("promptforge/input-broker"),
        "Debug lists the provided service ids: {services:?}"
    );

    let services = RunServices::with_host(
        promptforge::vfs::VfsRef::default(),
        CancelHandle::new(),
        host_with_broker(Scripted("typed")),
    );
    let broker = services
        .get(&INPUT_BROKER)
        .expect("the host's services supply the broker");
    assert_eq!(broker.wait().await.expect("the broker answers"), "typed");
    assert!(
        format!("{services:?}").contains("promptforge/input-broker"),
        "Debug lists the provided service ids: {services:?}"
    );
}

#[test]
fn the_input_service_is_provided_exactly_when_a_broker_is_present() {
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    assert!(!services.provides(&INPUT_BROKER.id()));
    let services = RunServices::with_host(
        promptforge::vfs::VfsRef::default(),
        CancelHandle::new(),
        host_with_broker(Scripted("typed")),
    );
    assert!(services.provides(&INPUT_BROKER.id()));
}

#[tokio::test]
async fn a_second_input_broker_is_refused_and_the_first_stays() {
    const SAME_LITERAL: ServiceKey<str> = ServiceKey::new("promptforge/input-broker");
    let mut host = host_with_broker(Scripted("first"));
    let second: Arc<dyn InputBroker> = Arc::new(Scripted("second"));
    assert!(
        matches!(
            host.provide(&INPUT_BROKER, second),
            Err(ServiceError::DuplicateId { .. })
        ),
        "a second broker under the id is refused"
    );
    assert!(
        matches!(
            host.provide(&SAME_LITERAL, Arc::from("the host's")),
            Err(ServiceError::DuplicateId { .. })
        ),
        "a provider of another type under the same literal is refused"
    );
    let services = RunServices::with_host(
        promptforge::vfs::VfsRef::default(),
        CancelHandle::new(),
        host,
    );
    let broker = services
        .get(&INPUT_BROKER)
        .expect("the first broker is provided");
    assert_eq!(
        broker.wait().await.expect("the broker answers"),
        "first",
        "the first provider stays"
    );
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
