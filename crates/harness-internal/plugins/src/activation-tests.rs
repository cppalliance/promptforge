//! Tests for activation by service id: a need is met only by a provider
//! under the id with the id's type, and an unmet need refuses the
//! declared Plugin, naming the id.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use promptforge::cancel::CancelHandle;
use promptforge::plugins::PluginId;
use promptforge::{MissingService, Prompt};

use super::{Activation, activate};
use crate::{
    Contribution, Plugin, PluginError, PluginRegistry, RunServices, ServiceId, ServiceKey,
};

/// The service the fixture needs.
const CLOCK: ServiceKey<str> = ServiceKey::new("acme/clock");

/// A key with the clock's literal and another type.
const CLOCK_AS_NUMBER: ServiceKey<u64> = ServiceKey::new("acme/clock");

fn timed_id() -> PluginId {
    PluginId::parse("timed").expect("the fixture id is valid")
}

/// A fixture Plugin that needs [`CLOCK`] and counts its activations.
struct Timed {
    id: PluginId,
    creates: Arc<AtomicUsize>,
}

impl Plugin for Timed {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture Plugin that needs a clock."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[CLOCK.id()];
        NEEDS
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, PluginError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(Contribution::default())
    }
}

/// Run services with nothing provided.
fn bare() -> RunServices {
    RunServices::new(CancelHandle::new())
}

/// Run services providing the clock under its own type.
fn with_clock() -> RunServices {
    let mut services = bare();
    services
        .host
        .provide(&CLOCK, Arc::from("noon"))
        .expect("the clock is accepted");
    services
}

/// Run services providing a number under the clock's id.
fn with_wrong_typed_clock() -> RunServices {
    let mut services = bare();
    services
        .host
        .provide(&CLOCK_AS_NUMBER, Arc::new(12))
        .expect("the number is accepted");
    services
}

/// Activates a prompt declaring `timed` under `services`. Returns the
/// activation and how many times the fixture's `create` ran.
fn activate_timed(services: &RunServices) -> (Activation, usize) {
    let source = "---\nname: timed\ndescription: d\npromptforge: 0\nplugins:\n  - timed\n---\n\n\
         # Title\n\n## Only\n\nDone.\n";
    let prompt = Prompt::parse(source, "timed")
        .0
        .expect("the fixture prompt parses");
    let creates = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(Timed {
            id: timed_id(),
            creates: Arc::clone(&creates),
        }))
        .expect("the fixture registers");
    let activation = activate(Some(&registry), &prompt, services);
    (activation, creates.load(Ordering::SeqCst))
}

/// The refusal `timed` gets without its clock.
fn clock_refusal() -> [MissingService; 1] {
    [MissingService::new(timed_id(), "acme/clock")]
}

#[test]
fn a_provider_of_the_needed_type_satisfies_the_need() {
    let (activation, creates) = activate_timed(&with_clock());
    assert_eq!(creates, 1, "create ran once");
    assert!(activation.requirements.is_satisfied());
}

#[test]
fn a_plugin_without_its_service_is_refused_naming_the_id() {
    let (activation, creates) = activate_timed(&bare());
    assert_eq!(creates, 0, "activation refuses before create runs");
    assert_eq!(activation.requirements.missing_services, clock_refusal());
    assert!(activation.requirements.missing_required.is_empty());
}

#[test]
fn a_wrong_typed_provider_counts_as_missing() {
    let (activation, creates) = activate_timed(&with_wrong_typed_clock());
    assert_eq!(creates, 0, "activation refuses before create runs");
    assert_eq!(activation.requirements.missing_services, clock_refusal());
}
