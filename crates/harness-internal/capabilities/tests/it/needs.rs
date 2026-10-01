//! Declared service needs at activation: a capability that needs the
//! input service, declared required or optional, on a Host that has or
//! lacks a broker.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use harness_capabilities::{
    Activation, Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution,
    InputBroker, InputError, RunServices, Service, activate,
};
use promptforge::cancel::CancelHandle;
use promptforge::vfs::VfsRef;
use promptforge::{MissingService, RunErrorKind, RunResult};

use super::support::{captured_logs, context, parse, run_activated};

/// A prompt declaring `acme/asker` as a required capability.
const REQUIRES_ASKER: &str = concat!(
    "---\n",
    "name: requires-asker\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - acme/asker\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `acme/asker` as an optional capability.
const OPTIONAL_ASKER: &str = concat!(
    "---\n",
    "name: optional-asker\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - ref: acme/asker\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

fn asker_id() -> CapabilityId {
    CapabilityId::parse("acme/asker").expect("the fixture id is valid")
}

/// A fixture capability that needs the input service and counts how
/// often its `create` runs.
struct Asker {
    id: CapabilityId,
    creates: Arc<AtomicUsize>,
}

impl Capability for Asker {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture capability that needs an input broker."
    }

    fn needs(&self) -> &[Service] {
        &[Service::Input]
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
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

fn registry_with_asker() -> (CapabilityRegistry, Arc<AtomicUsize>) {
    let creates = Arc::new(AtomicUsize::new(0));
    let mut registry = CapabilityRegistry::new();
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
    let mut services = RunServices::new(VfsRef::default(), CancelHandle::new());
    if with_broker {
        services = services.with_input(Arc::new(Silent));
    }
    let activation = activate(Some(&registry), &prompt, &services);
    (activation, creates.load(Ordering::SeqCst))
}

#[test]
fn a_required_capability_whose_service_is_present_activates_normally() {
    let (activation, creates) = activate_asker(REQUIRES_ASKER, true);
    assert_eq!(creates, 1, "create ran exactly once");
    assert!(activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn an_optional_capability_whose_service_is_present_activates_normally() {
    let (activation, creates) = activate_asker(OPTIONAL_ASKER, true);
    assert_eq!(creates, 1, "create ran exactly once");
    assert!(activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn a_required_capability_whose_service_is_missing_is_reported_without_calling_create() {
    let (activation, creates) = activate_asker(REQUIRES_ASKER, false);
    assert_eq!(
        creates, 0,
        "activation refuses before any capability code runs"
    );
    assert_eq!(
        activation.requirements.missing_services,
        [MissingService::new(asker_id(), "an input broker")]
    );
    assert!(
        activation.requirements.missing_required.is_empty(),
        "the capability is present, so it is not reported missing"
    );
    assert!(!activation.requirements.is_satisfied());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn an_optional_capability_whose_service_is_missing_activates_degraded_with_a_warning() {
    let logs = captured_logs(|| {
        let (activation, creates) = activate_asker(OPTIONAL_ASKER, false);
        assert_eq!(creates, 1, "an optional capability still activates");
        assert!(
            activation.requirements.is_satisfied(),
            "an optional capability's missing service does not refuse the run"
        );
        assert_eq!(activation.service_gaps.len(), 1);
        assert_eq!(activation.service_gaps[0].capability, asker_id());
        assert_eq!(activation.service_gaps[0].service, Service::Input);
    });
    assert!(
        logs.contains("WARN"),
        "the gap is logged as a warning: {logs}"
    );
    assert!(
        logs.contains("acme/asker") && logs.contains("an input broker"),
        "the warning names the capability and the service: {logs}"
    );
}

#[test]
fn the_run_path_refuses_a_required_capability_whose_service_is_missing() {
    let (registry, creates) = registry_with_asker();
    let prompt = parse(REQUIRES_ASKER, "requires-asker");
    // The suite's run path supplies no broker.
    let result = run_activated(&registry, &prompt, context("refuse-missing-service"));
    let RunResult::Failure(error) = result else {
        panic!("a required capability without its service is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - acme/asker needs an input broker, and this host provides none"
    );
    assert_eq!(creates.load(Ordering::SeqCst), 0);
}
