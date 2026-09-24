//! Tests for replay `Flags` and `ReplayError` rendering.

use super::{Flags, ReplayError};

#[test]
fn flags_start_empty_and_round_trip_their_bits() {
    assert!(Flags::EMPTY.is_empty());
    assert_eq!(Flags::default(), Flags::EMPTY);
    assert_eq!(Flags::EMPTY.bits(), 0);

    // A recorded run may set a bit this build does not name yet; the set
    // preserves it rather than dropping it, so replay can still see it.
    let recorded = Flags::from_bits(0b101);
    assert!(!recorded.is_empty());
    assert_eq!(recorded.bits(), 0b101);
    assert!(recorded.contains(Flags::from_bits(0b001)));
    assert!(recorded.contains(Flags::from_bits(0b100)));
    assert!(!recorded.contains(Flags::from_bits(0b010)));
    assert_eq!(Flags::from_bits(0b001) | Flags::from_bits(0b100), recorded);
}

#[test]
fn flags_serialize_as_one_integer() {
    let recorded = Flags::from_bits(6);
    assert_eq!(
        serde_json::to_string(&recorded).expect("flags serialize"),
        "6"
    );
    assert_eq!(
        serde_json::from_str::<Flags>("6").expect("flags deserialize"),
        recorded
    );
    assert_eq!(
        serde_json::to_string(&Flags::EMPTY).expect("flags serialize"),
        "0"
    );
}

#[test]
fn replay_errors_name_their_kind_and_detail() {
    fn assert_error<E: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<ReplayError>();

    let diverged = ReplayError::Nondeterminism {
        detail: "task 0.1 issued a chat effect where the record holds a tool call".to_owned(),
    };
    assert_eq!(
        diverged.to_string(),
        "replay diverged from its record: task 0.1 issued a chat effect where the record holds a tool call"
    );
    let malformed = ReplayError::Fatal {
        detail: "effect 7 has two answers".to_owned(),
    };
    assert_eq!(
        malformed.to_string(),
        "replay record is malformed: effect 7 has two answers"
    );
}
