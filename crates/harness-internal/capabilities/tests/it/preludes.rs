//! Preludes at activation: each activated capability's prelude is
//! returned in declaration order, and a capability that does not
//! activate (absent, in a conflicting pair, or failing in `create`)
//! contributes none.

use std::sync::Arc;

use harness_capabilities::{
    Activation, Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution,
    RunServices, activate,
};
use promptforge::cancel::CancelHandle;
use promptforge::capabilities::Prelude;

use super::support::parse;

fn id(text: &str) -> CapabilityId {
    CapabilityId::parse(text).expect("the fixture id is valid")
}

/// A fixture capability contributing `prelude` when it has one and no
/// tools. It conflicts with each id in `conflicts`, and `fail` turns its
/// activation into an error.
struct Preluder {
    id: CapabilityId,
    prelude: Option<&'static str>,
    conflicts: Vec<CapabilityId>,
    fail: bool,
}

impl Preluder {
    fn new(capability: &str, prelude: Option<&'static str>) -> Preluder {
        Preluder {
            id: id(capability),
            prelude,
            conflicts: Vec::new(),
            fail: false,
        }
    }

    fn conflicting_with(mut self, other: &str) -> Preluder {
        self.conflicts.push(id(other));
        self
    }

    fn failing(mut self) -> Preluder {
        self.fail = true;
        self
    }
}

impl Capability for Preluder {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture capability contributing a prelude."
    }

    fn conflicts(&self) -> &[CapabilityId] {
        &self.conflicts
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, CapabilityError> {
        if self.fail {
            return Err(CapabilityError::message("the fixture cannot activate"));
        }
        Ok(Contribution {
            tools: Vec::new(),
            prelude: self.prelude.map(str::to_owned),
        })
    }
}

/// Activates a prompt whose frontmatter declares `capabilities` (the
/// YAML list entries, one per line) against a registry holding
/// `installed`, on a Host with no input broker.
fn activate_declaring(capabilities: &str, installed: Vec<Preluder>) -> Activation {
    let source = format!(
        "---\nname: preludes\ndescription: d\npromptforge: 0\ncapabilities:\n{capabilities}\
         ---\n\n# Title\n\n## Only\n\nDone.\n"
    );
    let prompt = parse(&source, "preludes");
    let mut registry = CapabilityRegistry::new();
    for capability in installed {
        registry
            .register(Arc::new(capability))
            .expect("the fixture registers");
    }
    let services = RunServices::new(CancelHandle::new());
    activate(Some(&registry), &prompt, &services)
}

#[test]
fn activation_returns_each_contributed_prelude_in_declaration_order() {
    // The registry keeps its capabilities sorted by id, so declaring them
    // in reverse order tells declaration order apart from registry order.
    let activation = activate_declaring(
        "  - acme/omega\n  - acme/quiet\n  - acme/alpha\n",
        vec![
            Preluder::new("acme/alpha", Some("alpha = {}")),
            Preluder::new("acme/quiet", None),
            Preluder::new("acme/omega", Some("omega = {}")),
        ],
    );
    assert!(activation.requirements.is_satisfied());
    assert_eq!(
        activation.preludes,
        [
            Prelude::new(id("acme/omega"), "omega = {}"),
            Prelude::new(id("acme/alpha"), "alpha = {}"),
        ],
        "one prelude per contributing capability, in declaration order"
    );
}

#[test]
fn an_absent_capability_contributes_no_prelude() {
    let activation = activate_declaring(
        "  - acme/alpha\n  - ref: acme/absent\n    optional: true\n",
        vec![Preluder::new("acme/alpha", Some("alpha = {}"))],
    );
    assert!(activation.requirements.is_satisfied());
    assert_eq!(
        activation.preludes,
        [Prelude::new(id("acme/alpha"), "alpha = {}")]
    );
}

#[test]
fn a_conflicting_pair_contributes_no_prelude() {
    let activation = activate_declaring(
        "  - acme/left\n  - acme/right\n  - acme/alpha\n",
        vec![
            Preluder::new("acme/left", Some("left = {}")).conflicting_with("acme/right"),
            Preluder::new("acme/right", Some("right = {}")),
            Preluder::new("acme/alpha", Some("alpha = {}")),
        ],
    );
    assert_eq!(
        activation.requirements.conflicts.len(),
        1,
        "the pair is reported as a conflict"
    );
    assert_eq!(
        activation.preludes,
        [Prelude::new(id("acme/alpha"), "alpha = {}")],
        "neither member of the conflicting pair contributes its prelude"
    );
}

#[test]
fn a_capability_whose_create_fails_contributes_no_prelude() {
    let activation = activate_declaring(
        "  - ref: acme/broken\n    optional: true\n  - acme/alpha\n",
        vec![
            Preluder::new("acme/broken", Some("broken = {}")).failing(),
            Preluder::new("acme/alpha", Some("alpha = {}")),
        ],
    );
    assert!(activation.requirements.is_satisfied());
    assert_eq!(
        activation.preludes,
        [Prelude::new(id("acme/alpha"), "alpha = {}")]
    );
}
