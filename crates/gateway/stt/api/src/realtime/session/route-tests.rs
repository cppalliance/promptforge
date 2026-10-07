//! Tests for sample-to-millisecond conversion at the u64 boundary, for
//! hypothesis revisions that advance only when the emitted snapshot changes,
//! and for negotiated finalized range fields.

use std::ops::Range;

use base64::Engine as _;
use gateway_stt_engine::DecodeOutput;
use serde_json::Value;

use super::{InterimTaskOutput, Session, sample_millis};
use crate::realtime::registry::SessionRegistry;
use crate::realtime::session::InterimEpoch;

const HYPOTHESIS: &str = "item.input_audio_transcription.hypothesis";
const RANGES: &str = "item.input_audio_transcription.hypothesis.ranges";

#[test]
fn sample_milliseconds_are_exact_across_the_prior_multiplication_overflow() {
    assert_eq!(sample_millis(18_446_744_073_709_550), 1_152_921_504_606_846);
    assert_eq!(sample_millis(18_446_744_073_709_551), 1_152_921_504_606_846);
    assert_eq!(sample_millis(18_446_744_073_709_552), 1_152_921_504_606_847);
    assert_eq!(sample_millis(18_446_744_073_709_553), 1_152_921_504_606_847);
    assert_eq!(sample_millis(u64::MAX), 1_152_921_504_606_846_975);
}

fn append_committable(session: &mut Session) {
    session
        .append_base64(&base64::engine::general_purpose::STANDARD.encode([0_u8; 4_800]))
        .expect("committable audio appends");
}

fn session_including(include: &[&str]) -> Session {
    let registration = SessionRegistry::default()
        .register()
        .expect("session registers");
    let mut session = Session::new(registration, None);
    session
        .update_text(
            &serde_json::json!({
                "type": "session.update",
                "session": {
                    "type": "transcription",
                    "include": include
                }
            })
            .to_string(),
        )
        .expect("hypothesis include applies");
    append_committable(&mut session);
    session
}

fn hypothesis_session() -> Session {
    session_including(&[HYPOTHESIS])
}

fn accept_event(
    session: &mut Session,
    epoch: InterimEpoch,
    window: Range<u64>,
    transcript: &str,
) -> Option<Value> {
    let item_id = session.input().expect("input exists").item_id().to_owned();
    session
        .accept_scheduled_interim(InterimTaskOutput::Decode {
            epoch,
            item_id,
            segment_start: 0,
            audio_start: window.start,
            audio_end: window.end,
            transcript: Ok(DecodeOutput::new(transcript)),
        })
        .expect("interim is accepted")
        .map(|event| serde_json::to_value(event).expect("hypothesis serializes"))
}

fn accept(
    session: &mut Session,
    epoch: InterimEpoch,
    window: Range<u64>,
    transcript: &str,
) -> Option<u64> {
    accept_event(session, epoch, window, transcript).map(|event| {
        event["revision"]
            .as_u64()
            .expect("hypothesis carries a revision")
    })
}

#[test]
fn an_unchanged_snapshot_keeps_the_current_revision() {
    let mut session = hypothesis_session();
    let epoch = session.begin_interim().expect("epoch begins");
    assert_eq!(
        accept(&mut session, epoch, 0..16_000, "alpha beta"),
        Some(1)
    );
    assert_eq!(
        accept(&mut session, epoch, 0..24_000, "alpha beta"),
        Some(2),
        "agreement half a second later moves the text from tentative to agreed"
    );
    assert_eq!(
        accept(&mut session, epoch, 0..32_000, "alpha beta"),
        Some(2)
    );
}

#[test]
fn a_suppressed_snapshot_keeps_the_revision_and_the_next_change_takes_the_next_number() {
    let mut session = hypothesis_session();
    let epoch = session.begin_interim().expect("epoch begins");
    assert_eq!(
        accept(&mut session, epoch, 0..16_000, "alpha beta"),
        Some(1)
    );
    assert_eq!(
        accept(&mut session, epoch, 8_000..24_000, "zulu yankee"),
        None,
        "a sliding window with no rebase overlap is suppressed"
    );
    assert_eq!(session.hypothesis_revision, 1);
    assert_eq!(
        accept(&mut session, epoch, 0..20_000, "alpha beta gamma"),
        Some(2)
    );
}

#[test]
fn a_new_input_after_clear_or_commit_starts_again_at_revision_one() {
    let mut session = hypothesis_session();
    let epoch = session.begin_interim().expect("epoch begins");
    assert_eq!(
        accept(&mut session, epoch, 0..16_000, "alpha beta"),
        Some(1)
    );
    session.clear().expect("input clears");

    append_committable(&mut session);
    let epoch = session.begin_interim().expect("epoch begins after clear");
    assert_eq!(
        accept(&mut session, epoch, 0..16_000, "alpha beta"),
        Some(1)
    );
    let receipt = session.commit().expect("input commits");
    session.take_pending_interim(receipt.item_id());

    append_committable(&mut session);
    let epoch = session.begin_interim().expect("epoch begins after commit");
    assert_eq!(
        accept(&mut session, epoch, 0..16_000, "alpha beta"),
        Some(1)
    );
}

#[test]
fn range_fields_appear_only_for_a_ranges_input_and_report_the_finalized_text() {
    let mut base = hypothesis_session();
    let epoch = base.begin_interim().expect("epoch begins");
    let event = accept_event(&mut base, epoch, 0..16_000, "alpha beta")
        .expect("base hypothesis is emitted");
    assert!(event.get("finalized_through_ms").is_none());
    assert!(event.get("finalized_seq").is_none());

    let mut ranges = session_including(&[HYPOTHESIS, RANGES]);
    ranges
        .input()
        .expect("input exists")
        .take()
        .record_finalized_through("ask not", Some(16_000));
    let epoch = ranges.begin_interim().expect("epoch begins");
    let event = accept_event(&mut ranges, epoch, 16_000..32_000, "what you")
        .expect("ranges hypothesis is emitted");
    assert_eq!(event["finalized"], "ask not");
    assert_eq!(event["finalized_through_ms"], 1_000);
    assert_eq!(event["finalized_seq"], 1);
}

#[test]
fn only_a_ranges_session_advances_the_revision_for_a_range_only_change() {
    for (include, expected) in [(&[HYPOTHESIS][..], 2), (&[HYPOTHESIS, RANGES][..], 3)] {
        let mut session = session_including(include);
        let epoch = session.begin_interim().expect("epoch begins");
        assert_eq!(
            accept(&mut session, epoch, 0..16_000, "alpha beta"),
            Some(1)
        );
        assert_eq!(
            accept(&mut session, epoch, 0..24_000, "alpha beta"),
            Some(2)
        );
        session
            .input()
            .expect("input exists")
            .take()
            .record_finalized_through("", None);
        assert_eq!(
            accept(&mut session, epoch, 0..32_000, "alpha beta"),
            Some(expected),
            "{include:?}"
        );
    }
}
