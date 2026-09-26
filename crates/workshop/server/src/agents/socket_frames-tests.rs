//! Table-driven tests for the agent socket's framing helpers: the pure
//! render functions that map harness vocabulary onto Workshop wire
//! shapes, and the cursor and wire-index bookkeeping behind every
//! durable `agent_event` frame.

use harness::{Delta, SessionEvent, WaitFrame};
use promptforge::event::Event;
use promptforge::ids::{ChainId, Provenance, TaskId};
use workshop_protocol::InputFrame;

use super::{advance, delta_frame, input_frame};
use crate::agents::wire::{AgentDeltaFrame, AgentDeltaKind, AgentEventFrame};

#[test]
fn input_frames_map_harness_waits_to_wire_shapes() {
    let cases = [
        (
            WaitFrame::Required {
                token: "token-a".to_owned(),
            },
            InputFrame::Required {
                token: "token-a".to_owned(),
            },
        ),
        (
            WaitFrame::Cancelled {
                token: "token-b".to_owned(),
            },
            InputFrame::Cancelled {
                token: "token-b".to_owned(),
            },
        ),
    ];

    for (wait, expected) in cases {
        assert_eq!(input_frame(wait), expected);
    }
}

#[test]
fn delta_frames_map_harness_channels_to_wire_shapes() {
    // `Delta` is `#[non_exhaustive]` in `harness-sessions`, so each
    // fixture is deserialized rather than written as a struct literal.
    let cases = [
        (
            r#"{"kind":"text","content":"po","reply":2}"#,
            Some(AgentDeltaFrame::new(
                AgentDeltaKind::Text,
                "po".to_owned(),
                2,
            )),
        ),
        (
            r#"{"kind":"reasoning","content":"hmm","reply":3}"#,
            Some(AgentDeltaFrame::new(
                AgentDeltaKind::Reasoning,
                "hmm".to_owned(),
                3,
            )),
        ),
    ];

    for (json, expected) in cases {
        let delta: Delta = serde_json::from_str(json).expect("the delta fixture parses");
        assert_eq!(delta_frame(delta), expected);
    }
}

/// The root task's zeroth sequence: the wire does not expose it.
fn provenance() -> Provenance {
    Provenance {
        task: TaskId::from(ChainId::root()),
        seq: 0,
    }
}

/// A user-input event, which has a wire shape.
fn user_input(text: &str) -> Event {
    Event::UserInput {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        text: text.to_owned(),
    }
}

/// A transcript entry holding `event` in its persisted shape.
fn entry(index: u64, reply: Option<u64>, event: &Event) -> SessionEvent {
    SessionEvent {
        index,
        reply,
        event: serde_json::to_value(event).expect("an engine event serializes"),
    }
}

/// The frame `event` renders at wire index `index`.
fn frame(index: u64, reply: Option<u64>, event: &Event) -> AgentEventFrame {
    AgentEventFrame::new(index, reply, event).expect("a user-input event frames")
}

#[test]
fn framed_entries_take_gap_free_wire_indices_past_unframed_ones() {
    let hello = user_input("hello");
    let again = user_input("again");
    let lifecycle = Event::SectionStarted {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
    };
    let entries = [
        entry(0, None, &hello),
        entry(1, None, &lifecycle),
        SessionEvent {
            index: 2,
            reply: None,
            event: serde_json::json!({ "kind": "from_a_later_build" }),
        },
        entry(3, Some(1), &again),
    ];
    let (mut cursor, mut framed) = (0, 0);

    let rendered: Vec<_> = entries
        .iter()
        .map(|entry| advance(&mut cursor, &mut framed, entry))
        .collect();

    assert_eq!(
        rendered,
        [
            Some(frame(0, None, &hello)),
            None,
            None,
            Some(frame(1, Some(1), &again)),
        ],
        "a lifecycle entry and an unreadable payload frame nothing and take no wire index"
    );
    assert_eq!(cursor, 4, "the cursor moves past every entry read");
    assert_eq!(framed, 2, "only framed entries take a wire index");
}

#[test]
fn an_entry_below_the_cursor_moves_neither_cursor() {
    let (mut cursor, mut framed) = (5, 3);

    let replayed = advance(
        &mut cursor,
        &mut framed,
        &entry(4, None, &user_input("seen")),
    );

    assert_eq!(replayed, None, "an entry already read never frames twice");
    assert_eq!((cursor, framed), (5, 3));
}

#[test]
fn a_live_entry_past_a_gap_moves_the_cursor_beyond_it() {
    let late = user_input("late");
    let (mut cursor, mut framed) = (2, 1);

    let live = advance(&mut cursor, &mut framed, &entry(6, None, &late));

    assert_eq!(
        live,
        Some(frame(1, None, &late)),
        "the wire index counts frames sent, not transcript entries"
    );
    assert_eq!((cursor, framed), (7, 2));
}
