//! Tests for the Plugin activation contract.

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::plugins::PluginId;

use super::{Contribution, Plugin, PluginError, PluginErrorKind, RunServices};
use crate::{HostServices, INPUT_BROKER, InputBroker, InputError, ServiceError, ServiceKey};

/// A minimal in-process Plugin: a static id, no contributed tools, and
/// a `create` that refuses a cancelled run so tests can observe the
/// services it was handed.
struct StubPlugin {
    id: PluginId,
    description: String,
}

impl StubPlugin {
    fn web() -> StubPlugin {
        StubPlugin {
            id: PluginId::parse("promptforge/web").expect("a static valid id"),
            description: "A stub Plugin that contributes nothing.".to_owned(),
        }
    }
}

impl Plugin for StubPlugin {
    fn id(&self) -> &PluginId {
        &self.id
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, PluginError> {
        if services.cancel.is_cancelled() {
            return Err(PluginError::message("activation cancelled before create")
                .with_kind(PluginErrorKind::Cancelled));
        }
        Ok(Contribution::default())
    }
}

/// Compile-time proof that a Plugin can be shared across tasks and
/// threads behind a trait object: the registry stores `Arc<dyn Plugin>`.
const fn _assert_plugin_trait_object_is_shareable() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Arc<dyn Plugin>>();
}

#[test]
fn a_plugin_declares_no_conflicts_by_default() {
    let plugin = StubPlugin::web();
    assert!(plugin.conflicts().is_empty());
}

#[test]
fn a_plugin_needs_no_host_service_by_default() {
    let plugin = StubPlugin::web();
    assert!(plugin.needs().is_empty());
}

#[test]
fn a_plugin_is_object_safe_and_exposes_its_identity() {
    let plugin: Arc<dyn Plugin> = Arc::new(StubPlugin::web());
    assert_eq!(plugin.id().to_string(), "promptforge/web");
    assert!(!plugin.description().is_empty());
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
    let plugin = StubPlugin::web();
    let services = RunServices::new(CancelHandle::new());
    let contribution = plugin
        .create(&services)
        .expect("activation succeeds on a live run");
    assert!(contribution.tools.is_empty());

    let cancel = CancelHandle::new();
    cancel.cancel();
    let services = RunServices::new(cancel);
    let error = plugin
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
    let services = RunServices::new(CancelHandle::new());
    assert!(
        services.get(&INPUT_BROKER).is_none(),
        "a host that supplies no broker leaves the run without one"
    );
    assert!(
        !format!("{services:?}").contains("promptforge/input-broker"),
        "Debug lists the provided service ids: {services:?}"
    );

    let services = RunServices::with_host(CancelHandle::new(), host_with_broker(Scripted("typed")));
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
    let services = RunServices::new(CancelHandle::new());
    assert!(!services.provides(&INPUT_BROKER.id()));
    let services = RunServices::with_host(CancelHandle::new(), host_with_broker(Scripted("typed")));
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
    let services = RunServices::with_host(CancelHandle::new(), host);
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
fn plugin_error_display_is_the_model_readable_message() {
    let error = PluginError::message("the fs Plugin needs a writable store");
    assert_eq!(error.to_string(), "the fs Plugin needs a writable store");
    assert_eq!(error.kind(), PluginErrorKind::Other);
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn plugin_error_classifies_and_hides_its_cause() {
    let io = std::io::Error::other("disk full");
    let error = PluginError::with_source("activation failed", io);
    assert_eq!(error.kind(), PluginErrorKind::Activation);
    assert_eq!(error.to_string(), "activation failed");
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause stays behind Error::source, out of the model-readable message"
    );

    let cancelled = PluginError::message("stopped").with_kind(PluginErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());
    assert!(!PluginError::message("x").is_cancelled());
}
