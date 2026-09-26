//! Fixtures for other harness crates' tests, behind the `test-support`
//! feature. Nothing here is compiled into the harness proper.

use std::sync::Arc;

use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, Step};

use crate::spawn::Tag;

/// The tag a test's mock server is spawned under: the id and provenance
/// of a real issued effect, the one input wait a `user_input()` section
/// parks on.
///
/// The harness spawns only through [`crate::spawn::spawn_tagged`], and
/// the wrapper tags with an effect, so a mock server borrows one. Every
/// call builds a fresh throwaway run, so the tag is the same each time
/// and the run it came from is dropped at once.
///
/// # Panics
///
/// Panics when the fixture prompt fails to parse or does not park on an
/// input wait, which would be a regression in the engine.
#[must_use]
pub fn mock_tag() -> Tag {
    let source = "---\nname: mock\ndescription: a mock server's tag\npromptforge: 0\n---\n\n\
                  # Mock\n\n## Only\n\n```lua\nreturn user_input()\n```\n";
    let (prompt, _parse_events) = Prompt::parse(source, "mock");
    let Ok(prompt) = prompt else {
        panic!("the tag fixture parses");
    };
    let mut run = Run::new(
        Arc::new(prompt),
        "",
        RunContext::new("mock", 1, Timestamp::UNIX_EPOCH),
    );
    let Step::Pending { mut effects, .. } = run.step() else {
        panic!("the input wait leaves the run pending");
    };
    let (id, provenance, _effect) = effects.remove(0);
    (id, provenance)
}
