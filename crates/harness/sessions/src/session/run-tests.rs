//! Tests that run failures pushed to the client keep their cause chain.

use std::io;
use std::path::PathBuf;

use harness_runner::display_chain;
use harness_runner::prepare::PrepareError;

use super::RunFailure;

#[test]
fn a_prepare_failure_pushed_to_the_client_carries_its_cause_chain() {
    let failure = RunFailure::Prepare(PrepareError::Read {
        path: PathBuf::from("agent.md"),
        source: io::Error::other("disk gone"),
    });
    let rendered = display_chain(&failure);
    assert!(
        rendered.contains("could not be read"),
        "the outer frame names what happened: {rendered}"
    );
    assert!(
        rendered.contains("disk gone"),
        "the innermost cause reaches the client: {rendered}"
    );
    assert_eq!(
        rendered.matches("could not be read").count(),
        1,
        "the transparent wrapper does not double the preparation text: {rendered}"
    );
}
