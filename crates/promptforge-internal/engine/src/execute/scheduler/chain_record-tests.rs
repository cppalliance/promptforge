//! Tests for the chain's usage-anchor bookkeeping: what a settled round
//! keeps for the next precheck, what a failed or usage-less round leaves
//! alone, and which model a measurement is offered to.

use promptforge_types::metrics::Usage;

use super::ChatAnchor;
use crate::lua::UsageAnchor;
use crate::model::{Message, ModelId};

fn usage(prompt: u32, completion: u32) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        cached_tokens: None,
        reasoning_tokens: None,
    }
}

fn request(text: &str) -> Vec<Message> {
    vec![Message::user(text)]
}

fn model(name: &str) -> ModelId {
    ModelId::gateway(name).expect("the test model name is valid")
}

#[test]
fn a_chain_starts_with_no_measurement() {
    assert!(ChatAnchor::default().measured(&model("a")).is_none());
}

#[test]
fn a_round_that_reported_usage_becomes_the_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    let measured = anchor
        .measured(&model("a"))
        .expect("the round reported usage");
    assert_eq!(measured.tokens(), 120);
}

#[test]
fn the_newest_round_that_reported_usage_replaces_the_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(model("a"), request("second"));
    anchor.settle(Some(&usage(300, 40)));
    assert_eq!(
        anchor.measured(&model("a")).map(UsageAnchor::tokens),
        Some(340)
    );
}

#[test]
fn a_round_with_no_usage_keeps_the_older_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(model("a"), request("second"));
    anchor.settle(None);
    assert_eq!(
        anchor.measured(&model("a")).map(UsageAnchor::tokens),
        Some(120),
        "a round that measured nothing leaves the older measurement"
    );
}

#[test]
fn usage_with_no_round_in_flight_measures_nothing() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(None);
    // The failed round released its messages, so a late usage has nothing
    // to attach to.
    anchor.settle(Some(&usage(100, 20)));
    assert!(anchor.measured(&model("a")).is_none());
}

#[test]
fn the_measurement_is_offered_only_to_the_model_that_made_it() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    assert_eq!(
        anchor.measured(&model("a")).map(UsageAnchor::tokens),
        Some(120),
        "an id built again from the same parts is the same model"
    );
    assert!(
        anchor.measured(&model("b")).is_none(),
        "another model's tokenizer counted these tokens"
    );
    let other_server = ModelId::new("elsewhere", "a").expect("the test id is valid");
    assert!(
        anchor.measured(&other_server).is_none(),
        "the same name on another server is another model"
    );
}

#[test]
fn a_round_on_another_model_replaces_the_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(model("b"), request("second"));
    anchor.settle(Some(&usage(300, 40)));
    assert_eq!(
        anchor.measured(&model("b")).map(UsageAnchor::tokens),
        Some(340)
    );
    assert!(
        anchor.measured(&model("a")).is_none(),
        "the old model's measurement is gone once the new one settles"
    );
}

#[test]
fn a_round_on_another_model_with_no_usage_keeps_the_older_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(model("a"), request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(model("b"), request("second"));
    anchor.settle(None);
    assert_eq!(
        anchor.measured(&model("a")).map(UsageAnchor::tokens),
        Some(120),
        "a round that measured nothing leaves the older measurement"
    );
    assert!(anchor.measured(&model("b")).is_none());
}
