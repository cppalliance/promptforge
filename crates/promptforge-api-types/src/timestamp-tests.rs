//! Tests that the std-only RFC 3339 formatter agrees with the `time` crate.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::Timestamp;

/// The `time` crate's rendering of the same instant: the reference the
/// std-only formatter must agree with byte for byte.
fn reference(millis: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .expect("every sample is inside the time crate's range")
        .format(&Rfc3339)
        .expect("the time crate renders every sample")
}

#[test]
fn to_rfc3339_agrees_with_the_time_crate_on_a_table_including_leap_days() {
    let samples: [(i64, &str); 16] = [
        (0, "1970-01-01T00:00:00Z"),
        (-1, "1969-12-31T23:59:59.999Z"),
        (1_000, "1970-01-01T00:00:01Z"),
        // 1972-02-29: the first leap day after the epoch.
        (68_169_600_000, "1972-02-29T00:00:00Z"),
        // 1900 is not a leap year (divisible by 100, not by 400).
        (-2_203_891_200_000, "1900-03-01T00:00:00Z"),
        (-2_203_891_200_001, "1900-02-28T23:59:59.999Z"),
        // 2000 is a leap year (divisible by 400).
        (951_782_400_000, "2000-02-29T00:00:00Z"),
        (951_868_799_999, "2000-02-29T23:59:59.999Z"),
        (951_868_800_000, "2000-03-01T00:00:00Z"),
        // 2024-02-29 with every fraction width the millisecond grid allows.
        (1_709_210_096_789, "2024-02-29T12:34:56.789Z"),
        (1_709_210_096_780, "2024-02-29T12:34:56.78Z"),
        (1_709_210_096_700, "2024-02-29T12:34:56.7Z"),
        // 2100 is not a leap year.
        (4_107_542_399_000, "2100-02-28T23:59:59Z"),
        (4_107_542_400_000, "2100-03-01T00:00:00Z"),
        // The ends of the four-digit-year range.
        (-62_135_596_800_000, "0001-01-01T00:00:00Z"),
        (253_402_300_799_999, "9999-12-31T23:59:59.999Z"),
    ];
    for (millis, expected) in samples {
        let rendered = Timestamp::from_unix_millis(millis).to_rfc3339();
        assert_eq!(rendered, expected, "millis {millis}");
        assert_eq!(rendered, reference(millis), "millis {millis}");
    }
}

#[test]
fn to_rfc3339_agrees_with_the_time_crate_across_a_sweep() {
    // A stride that is coprime with a day, so the sweep lands on every hour,
    // minute, second, and fraction pattern across four centuries.
    let stride: i64 = 37 * 86_400_000 + 12_345_679;
    let mut millis: i64 = -6_311_347_200_000; // 1770-01-01T00:00:00Z
    while millis < 6_311_433_600_000 {
        // 2170-01-01T00:00:00Z
        assert_eq!(
            Timestamp::from_unix_millis(millis).to_rfc3339(),
            reference(millis),
            "millis {millis}"
        );
        millis += stride;
    }
}

#[test]
fn a_timestamp_is_its_millisecond_count_on_the_wire() {
    let stamp = Timestamp::from_unix_millis(1_709_210_096_789);
    assert_eq!(stamp.unix_millis(), 1_709_210_096_789);
    assert_eq!(
        serde_json::to_string(&stamp).expect("a timestamp serializes"),
        "1709210096789"
    );
    assert_eq!(
        serde_json::from_str::<Timestamp>("1709210096789").expect("a timestamp deserializes"),
        stamp
    );
    assert_eq!(stamp.to_string(), stamp.to_rfc3339());
    assert!(Timestamp::UNIX_EPOCH < stamp);
}
