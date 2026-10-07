//! Tests for the transcript: every event is numbered from zero, and a
//! round's content events take their `reply` from that round.

use promptforge::event::{Event, ReplyOrigin};
use promptforge::ids::{Provenance, RoundId};

use super::*;

/// The coordinates every event carries; only the kind and payload matter
/// to the numbering and the stamp.
fn at() -> (String, String, Provenance) {
    (
        "conversation-1".to_owned(),
        "Conversation".to_owned(),
        Provenance {
            task: "0".parse().unwrap(),
            seq: 0,
        },
    )
}

fn answered(content: &str) -> Event {
    let (execution, section, provenance) = at();
    Event::ToolResult {
        execution,
        section,
        provenance,
        turn: 0,
        tool_call_id: String::new(),
        alias: "ask".to_owned(),
        tool: None,
        content: content.to_owned(),
        trusted: true,
    }
}

fn thinking(round: u64) -> Event {
    let (execution, section, provenance) = at();
    Event::Thinking {
        execution,
        section,
        provenance,
        turn: 0,
        round: RoundId::new(round),
        model: "some-model".to_owned(),
        text: "weighing options".to_owned(),
    }
}

fn reply(round: u64, origin: ReplyOrigin) -> Event {
    let (execution, section, provenance) = at();
    Event::AssistantReply {
        execution,
        section,
        provenance,
        turn: 0,
        round: RoundId::new(round),
        text: "42".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "some-model".to_owned(),
        metrics: None,
        origin,
    }
}

fn tool_calls(round: u64) -> Event {
    let (execution, section, provenance) = at();
    Event::AssistantToolCalls {
        execution,
        section,
        provenance,
        turn: 0,
        round: RoundId::new(round),
        model: "some-model".to_owned(),
        calls: Vec::new(),
    }
}

#[test]
fn every_event_is_numbered_from_zero_in_the_order_it_lands() {
    let transcript = Transcript::new();
    let stamped: Vec<u64> = [answered("one"), thinking(0), reply(0, ReplyOrigin::Chat)]
        .iter()
        .map(|event| transcript.push(event).index)
        .collect();
    assert_eq!(stamped, [0, 1, 2], "the live entries number from zero");
    let read: Vec<u64> = transcript
        .since(0)
        .iter()
        .map(|entry| entry.index)
        .collect();
    assert_eq!(read, [0, 1, 2], "the transcript holds the same numbers");
    assert_eq!(
        transcript.since(1).first().map(|entry| entry.index),
        Some(1),
        "a read from an index starts there"
    );
    assert!(
        transcript.since(9).is_empty(),
        "a read past the end is empty"
    );
}

#[test]
fn a_rounds_content_events_take_their_reply_from_the_round() {
    let transcript = Transcript::new();
    // A nested infer round numbered 4 lands between a section's rounds 3
    // and 5: each event names its own round, whatever landed before it.
    let cases = [
        (thinking(3), Some(3)),
        (reply(3, ReplyOrigin::Chat), Some(3)),
        (reply(4, ReplyOrigin::Infer), Some(4)),
        (tool_calls(5), Some(5)),
        (answered("next"), None),
        (reply(5, ReplyOrigin::Chat), Some(5)),
    ];
    for (event, expected) in cases {
        let entry = transcript.push(&event);
        assert_eq!(entry.reply, expected, "{event:?}");
        assert_eq!(
            transcript.since(entry.index)[0].reply,
            expected,
            "the transcript keeps the stamp the live entry carried"
        );
    }
}

#[test]
fn an_entry_carries_the_event_in_its_persisted_shape() {
    let transcript = Transcript::new();
    let event = answered("ping");
    let entry = transcript.push(&event);
    assert_eq!(
        entry.event,
        serde_json::to_value(&event).expect("an event serializes"),
        "the entry's payload is the event the recorder took"
    );
}
