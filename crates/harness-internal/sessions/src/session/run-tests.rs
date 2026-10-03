//! Tests that run failures pushed to the client keep their cause chain,
//! and that the run's delta callback feeds a listening session and drops
//! deltas once the session stops listening.

use std::io;
use std::path::PathBuf;

use harness_runner::display_chain;
use harness_runner::prepare::PrepareError;
use promptforge::model::StreamDelta;
use tokio::sync::mpsc;

use super::{RunFailure, delta_callback};

#[test]
fn a_prepare_failure_pushed_to_the_client_keeps_its_cause_chain() {
    let failure = RunFailure::Prepare(Box::new(PrepareError::Read {
        path: PathBuf::from("agent.md"),
        source: io::Error::other("disk gone"),
    }));
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

#[test]
fn the_delta_callback_hands_each_delta_to_a_listening_session() {
    let (deltas, mut heard) = mpsc::unbounded_channel();
    let on_delta = delta_callback(deltas);

    on_delta(StreamDelta::Text("one ".to_owned()));
    on_delta(StreamDelta::Text("two".to_owned()));
    assert_eq!(
        heard.try_recv().ok(),
        Some(StreamDelta::Text("one ".to_owned()))
    );
    assert_eq!(
        heard.try_recv().ok(),
        Some(StreamDelta::Text("two".to_owned()))
    );
}

#[test]
fn a_session_that_stopped_listening_drops_deltas_without_failing_the_round() {
    let (deltas, heard) = mpsc::unbounded_channel();
    let on_delta = delta_callback(deltas.clone());
    drop(heard);
    assert!(deltas.is_closed(), "the session's receiver is gone");

    // A panic here would abort the round's future mid-stream; returning
    // is the callback dropping the delta.
    on_delta(StreamDelta::Text("unheard".to_owned()));
    on_delta(StreamDelta::Text("still unheard".to_owned()));
}
