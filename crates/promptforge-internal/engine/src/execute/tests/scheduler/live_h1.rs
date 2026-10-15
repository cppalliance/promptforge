//! The live H1 pass on the scheduler. Each submodule holds one topic and
//! reaches the scenario below through `use super::*`.

use super::*;

/// A run of `source` for a scheduler live-H1 test: unprepared, with every
/// declared role bound to the test model the way prepare's trivial fill
/// binds it, and the shared model set empty - the live H1 pass under test
/// records its own bindings.
fn h1_scenario(source: &str) -> Scenario {
    Scenario::new(source).unprepared().bind_roles()
}

mod control_flow;
mod pass;
mod returns_and_prose;
