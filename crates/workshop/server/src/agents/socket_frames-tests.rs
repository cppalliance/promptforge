//! Table-driven tests for the agent socket's framing helpers: the pure
//! render functions that map harness vocabulary onto Workshop wire
//! shapes. The durable-event helpers (`drain_events`, `frame_entry`)
//! need a live session and socket, so the integration socket tests cover
//! them end to end.

use harness_api::{Delta, WaitFrame};
use workshop_protocol::{AgentDeltaFrame, AgentDeltaKind, InputFrame};

use super::{delta_frame, input_frame};

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
