//! Tests for the reply-id stamp rule: the one rule a live event and a
//! transcript read share, so a reply of either origin - a user-facing chat
//! turn or a programmatic inference round - settles and advances the round
//! the same way.

use promptforge_api_types::event::ReplyOrigin;
use promptforge_api_types::ids::Provenance;

use super::*;

/// The coordinates every event carries; only the kind and payload matter
/// to the stamp rule.
fn at(execution: &str) -> (String, String, Provenance) {
    (
        execution.to_owned(),
        "Section".to_owned(),
        Provenance {
            task: "0".parse().unwrap(),
            seq: 0,
        },
    )
}

#[test]
fn a_reply_of_either_origin_stamps_the_current_round_and_advances() {
    for origin in [ReplyOrigin::Chat, ReplyOrigin::Infer] {
        let mut rounds_seen = 3u64;
        let (execution, section, provenance) = at("run-1");
        let reply = reply_stamp(
            &Event::AssistantReply {
                execution,
                section,
                provenance,
                turn: 3,
                text: "42".to_owned(),
                finish_reason: Some("stop".to_owned()),
                model: "some-model".to_owned(),
                metrics: None,
                origin,
            },
            &mut rounds_seen,
        );
        assert_eq!(
            reply,
            Some(3),
            "an {origin:?} reply carries the round it was produced under"
        );
        assert_eq!(
            rounds_seen, 4,
            "a settled {origin:?} round advances the count for the next round"
        );
    }
}

#[test]
fn thinking_stamps_the_current_round_without_advancing() {
    let mut rounds_seen = 3u64;
    let (execution, section, provenance) = at("run-1");
    let reply = reply_stamp(
        &Event::Thinking {
            execution,
            section,
            provenance,
            turn: 3,
            model: "some-model".to_owned(),
            text: "weighing options".to_owned(),
        },
        &mut rounds_seen,
    );
    assert_eq!(
        reply,
        Some(3),
        "thinking is stamped with the round it belongs to"
    );
    assert_eq!(
        rounds_seen, 3,
        "thinking does not settle a round, so the count is unchanged"
    );
}
