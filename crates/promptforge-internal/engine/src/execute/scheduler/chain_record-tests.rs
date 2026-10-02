//! Tests for the chain's usage-anchor bookkeeping: what a settled round
//! keeps for the next precheck, and what a failed or usage-less round
//! leaves alone.

use promptforge_types::metrics::Usage;

use super::ChatAnchor;
use crate::lua::UsageAnchor;
use crate::model::Message;

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

#[test]
fn a_chain_starts_with_no_measurement() {
    assert!(ChatAnchor::default().measured().is_none());
}

#[test]
fn a_round_that_reported_usage_becomes_the_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(request("first"));
    anchor.settle(Some(&usage(100, 20)));
    let measured = anchor.measured().expect("the round reported usage");
    assert_eq!(measured.tokens(), 120);
}

#[test]
fn the_newest_round_that_reported_usage_replaces_the_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(request("second"));
    anchor.settle(Some(&usage(300, 40)));
    assert_eq!(anchor.measured().map(UsageAnchor::tokens), Some(340));
}

#[test]
fn a_round_with_no_usage_keeps_the_older_measurement() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(request("first"));
    anchor.settle(Some(&usage(100, 20)));
    anchor.sending(request("second"));
    anchor.settle(None);
    assert_eq!(
        anchor.measured().map(UsageAnchor::tokens),
        Some(120),
        "a round that measured nothing leaves the older measurement"
    );
}

#[test]
fn usage_with_no_round_in_flight_measures_nothing() {
    let mut anchor = ChatAnchor::default();
    anchor.sending(request("first"));
    anchor.settle(None);
    // The failed round released its messages, so a late usage has nothing
    // to attach to.
    anchor.settle(Some(&usage(100, 20)));
    assert!(anchor.measured().is_none());
}
