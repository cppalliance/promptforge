//! Declared service needs at activation: a declared Plugin that needs
//! the input broker, on a Host that has or lacks one.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use harness_plugins::{
    Activation, Contribution, HostServices, INPUT_BROKER, InputBroker, InputError, Plugin,
    PluginError, PluginId, PluginRegistry, RunServices, ServiceId, activate,
};
use promptforge::cancel::CancelHandle;
use promptforge::{MissingService, RunErrorKind, RunResult};

use super::support::{context, parse, run_activated};

/// A prompt declaring `asker` as a required Plugin.
const REQUIRES_ASKER: &str = concat!(
    "---\n",
    "name: requires-asker\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - asker\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

fn asker_id() -> PluginId {
    PluginId::parse("asker").expect("the fixture id is valid")
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
         - asker needs promptforge/input-broker, and the environment provides none"
    );
    assert_eq!(creates.load(Ordering::SeqCst), 0);
}
