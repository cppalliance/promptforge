//! Declared service needs at activation: a Plugin that needs the
//! input broker, declared required or optional, on a Host that has or
//! lacks one.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use harness_plugins::{
    Activation, Contribution, HostServices, INPUT_BROKER, InputBroker, InputError, Plugin,
    PluginError, PluginId, PluginRegistry, RunServices, ServiceId, activate,
};
use promptforge::cancel::CancelHandle;
use promptforge::{MissingService, RunErrorKind, RunResult};

use super::support::{captured_logs, context, parse, run_activated};

/// A prompt declaring `acme/asker` as a required Plugin.
const REQUIRES_ASKER: &str = concat!(
    "---\n",
    "name: requires-asker\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - acme/asker\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `acme/asker` as an optional Plugin.
const OPTIONAL_ASKER: &str = concat!(
    "---\n",
    "name: optional-asker\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - ref: acme/asker\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

fn asker_id() -> PluginId {
    PluginId::parse("acme/asker").expect("the fixture id is valid")
}

/// A fixture Plugin that needs the input service and counts how
/// often its `create` runs.
struct Asker {
    id: PluginId,
    creates: Arc<AtomicUsize>,
}

impl Plugin for Asker {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture Plugin that needs an input broker."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[INPUT_BROKER.id()];
        NEEDS
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, PluginError> {
        let _ = services;
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(Contribution::default())
    }
}

/// A broker whose operator never has anything to say; these tests only
/// need one to be present.
struct Silent;

#[async_trait::async_trait]
impl InputBroker for Silent {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(String::new())
    }
}

fn registry_with_asker() -> (PluginRegistry, Arc<AtomicUsize>) {
    let creates = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(Asker {
            id: asker_id(),
            creates: Arc::clone(&creates),
        }))
        .expect("the fixture registers");
    (registry, creates)
}

/// Activates `source` against a registry holding the asker, on a Host
/// that has a broker when `with_broker` is set. Returns the
/// activation and how many times the asker's `create` ran.
fn activate_asker(source: &str, with_broker: bool) -> (Activation, usize) {
    let (registry, creates) = registry_with_asker();
    let prompt = parse(source, "asker");
    let mut host = HostServices::new();
    if with_broker {
        let broker: Arc<dyn InputBroker> = Arc::new(Silent);
        host.provide(&INPUT_BROKER, broker)
            .expect("an empty map takes the broker");
    }
    let services = RunServices::with_host(CancelHandle::new(), host);
    let activation = activate(Some(&registry), &prompt, &services);
    (activation, creates.load(Ordering::SeqCst))
}

#[test]
fn a_required_plugin_whose_service_is_present_activates_normally() {
    let (activation, creates) = activate_asker(REQUIRES_ASKER, true);
    assert_eq!(creates, 1, "create ran exactly once");
    assert!(activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn an_optional_plugin_whose_service_is_present_activates_normally() {
    let (activation, creates) = activate_asker(OPTIONAL_ASKER, true);
    assert_eq!(creates, 1, "create ran exactly once");
    assert!(activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn a_required_plugin_whose_service_is_missing_is_reported_without_calling_create() {
    let (activation, creates) = activate_asker(REQUIRES_ASKER, false);
    assert_eq!(creates, 0, "activation refuses before any Plugin code runs");
    assert_eq!(
        activation.requirements.missing_services,
        [MissingService::new(asker_id(), "promptforge/input-broker")]
    );
    assert!(
        activation.requirements.missing_required.is_empty(),
        "the Plugin is present, so it is not reported missing"
    );
    assert!(!activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn an_optional_plugin_whose_service_is_missing_activates_degraded_with_a_warning() {
    let logs = captured_logs(|| {
        let (activation, creates) = activate_asker(OPTIONAL_ASKER, false);
        assert_eq!(creates, 1, "an optional Plugin still activates");
        assert!(
            activation.requirements.is_satisfied(),
            "an optional Plugin's missing service does not refuse the run"
        );
        assert_eq!(activation.service_gaps.len(), 1);
        assert_eq!(activation.service_gaps[0].plugin, asker_id());
        assert_eq!(activation.service_gaps[0].service, INPUT_BROKER.id());
    });
    assert!(
        logs.contains("WARN"),
        "the gap is logged as a warning: {logs}"
    );
    assert!(
        logs.contains("acme/asker") && logs.contains("promptforge/input-broker"),
        "the warning names the Plugin and the service: {logs}"
    );
}

#[test]
fn the_run_path_refuses_a_required_plugin_whose_service_is_missing() {
    let (registry, creates) = registry_with_asker();
    let prompt = parse(REQUIRES_ASKER, "requires-asker");
    // The suite's run path supplies no broker.
    let result = run_activated(&registry, &prompt, context("refuse-missing-service"));
    let RunResult::Failure(error) = result else {
        panic!("a required Plugin without its service is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - acme/asker needs promptforge/input-broker, and this host provides none"
    );
    assert_eq!(creates.load(Ordering::SeqCst), 0);
}
