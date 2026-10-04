//! Tests for activation by service id: a need is met only by a provider
//! under the id with the id's type, and an unmet need refuses a required
//! capability or records a gap for an optional one, naming the id.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;
use promptforge::{MissingService, Prompt};

use super::{Activation, ServiceGap, activate};
use crate::{
    Capability, CapabilityError, CapabilityRegistry, Contribution, RunServices, ServiceId,
    ServiceKey,
};

/// The service the fixture needs.
const CLOCK: ServiceKey<str> = ServiceKey::new("acme/clock");

/// A key with the clock's literal and another type.
const CLOCK_AS_NUMBER: ServiceKey<u64> = ServiceKey::new("acme/clock");

fn timed_id() -> CapabilityId {
    CapabilityId::parse("acme/timed").expect("the fixture id is valid")
}

/// A fixture capability that needs [`CLOCK`] and counts its activations.
struct Timed {
    id: CapabilityId,
    creates: Arc<AtomicUsize>,
}

impl Capability for Timed {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture capability that needs a clock."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[CLOCK.id()];
        NEEDS
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, CapabilityError> {
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

/// Activates a prompt declaring `acme/timed`, optional when `optional`
/// is set, under `services`. Returns the activation and how many times
/// the fixture's `create` ran.
fn activate_timed(optional: bool, services: &RunServices) -> (Activation, usize) {
    let declaration = if optional {
        "  - ref: acme/timed\n    optional: true\n"
    } else {
        "  - acme/timed\n"
    };
    let source = format!(
        "---\nname: timed\ndescription: d\npromptforge: 0\ncapabilities:\n{declaration}---\n\n\
         # Title\n\n## Only\n\nDone.\n"
    );
    let prompt = Prompt::parse(&source, "timed")
        .0
        .expect("the fixture prompt parses");
    let creates = Arc::new(AtomicUsize::new(0));
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Timed {
            id: timed_id(),
            creates: Arc::clone(&creates),
        }))
        .expect("the fixture registers");
    let activation = activate(Some(&registry), &prompt, services);
    (activation, creates.load(Ordering::SeqCst))
}

/// The refusal a required `acme/timed` gets without its clock.
fn clock_refusal() -> [MissingService; 1] {
    [MissingService::new(timed_id(), "acme/clock")]
}

/// The gap an optional `acme/timed` records without its clock.
fn clock_gap() -> [ServiceGap; 1] {
    [ServiceGap {
        capability: timed_id(),
        service: CLOCK.id(),
    }]
}

#[test]
fn a_provider_of_the_needed_type_satisfies_the_need() {
    for optional in [false, true] {
        let (activation, creates) = activate_timed(optional, &with_clock());
        assert_eq!(creates, 1, "optional {optional}: create ran once");
        assert!(activation.requirements.is_satisfied());
        assert!(activation.service_gaps.is_empty());
    }
}

#[test]
fn a_required_capability_without_its_service_is_refused_naming_the_id() {
    let (activation, creates) = activate_timed(false, &bare());
    assert_eq!(creates, 0, "activation refuses before create runs");
    assert_eq!(activation.requirements.missing_services, clock_refusal());
    assert!(activation.requirements.missing_required.is_empty());
    assert!(activation.service_gaps.is_empty());
}

#[test]
fn an_optional_capability_without_its_service_records_a_gap_naming_the_id() {
    let (activation, creates) = activate_timed(true, &bare());
    assert_eq!(creates, 1, "an optional capability still activates");
    assert!(activation.requirements.is_satisfied());
    assert_eq!(activation.service_gaps, clock_gap());
}

#[test]
fn a_wrong_typed_provider_counts_as_missing_for_a_required_capability() {
    let (activation, creates) = activate_timed(false, &with_wrong_typed_clock());
    assert_eq!(creates, 0, "activation refuses before create runs");
    assert_eq!(activation.requirements.missing_services, clock_refusal());
}

#[test]
fn a_wrong_typed_provider_counts_as_missing_for_an_optional_capability() {
    let (activation, creates) = activate_timed(true, &with_wrong_typed_clock());
    assert_eq!(creates, 1, "an optional capability still activates");
    assert!(activation.requirements.is_satisfied());
    assert_eq!(activation.service_gaps, clock_gap());
}
