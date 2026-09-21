//! A UTC instant in milliseconds, rendered to RFC 3339 over std alone.
//!
//! A run's `started_at` is an input the host draws, recorded in the run
//! log, and replayed verbatim; the clock belongs to the host.
//! [`Timestamp`] is the value that crosses that boundary. Its one
//! rendering, [`to_rfc3339`](Timestamp::to_rfc3339), is what a prompt
//! reads as `sys.when`; it is written over std so the engine takes no
//! clock or calendar dependency, and it agrees byte for byte with the
//! `time` crate's RFC 3339 rendering of the same instant (the tests hold
//! it to that).

use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "timestamp-tests.rs"]
mod tests;

/// A UTC instant: signed milliseconds since the Unix epoch.
///
/// Serializes as that integer. Orders chronologically.
///
/// # Examples
/// ```
/// use promptforge_api_types::timestamp::Timestamp;
///
/// let stamp = Timestamp::from_unix_millis(951_782_400_000);
/// assert_eq!(stamp.to_rfc3339(), "2000-02-29T00:00:00Z");
/// assert_eq!(stamp.unix_millis(), 951_782_400_000);
/// ```
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// `1970-01-01T00:00:00Z`.
    pub const UNIX_EPOCH: Timestamp = Timestamp(0);

    /// The instant `millis` milliseconds after the Unix epoch (before it
    /// when negative).
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.0
    }

    /// The instant `time` represents, saturated to [`Timestamp::UNIX_EPOCH`]
    /// if `time` is before the epoch or beyond `i64` milliseconds.
    #[must_use]
    pub fn from_system_time(time: std::time::SystemTime) -> Self {
        Self::from(time)
    }

    /// The current system clock as a [`Timestamp`].
    #[must_use]
    pub fn now() -> Self {
        Self::from(std::time::SystemTime::now())
    }

    /// The instant as an RFC 3339 UTC string: `2024-02-29T12:34:56.789Z`.
    ///
    /// The fraction is omitted when the millisecond count is zero and
    /// otherwise drops its trailing zeros (`.78`, `.7`), which is the
    /// `time` crate's rendering. Years outside `0000..=9999` render with
    /// more digits or a sign and are not RFC 3339; no run is stamped there.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        const MILLIS_PER_DAY: i64 = 86_400_000;
        let days = self.0.div_euclid(MILLIS_PER_DAY);
        let millis_of_day = self.0.rem_euclid(MILLIS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let seconds_of_day = millis_of_day / 1_000;
        let (hour, minute, second) = (
            seconds_of_day / 3_600,
            seconds_of_day % 3_600 / 60,
            seconds_of_day % 60,
        );
        let mut out = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
        let millis = millis_of_day % 1_000;
        if millis != 0 {
            let fraction = format!("{millis:03}");
            out.push('.');
            out.push_str(fraction.trim_end_matches('0'));
        }
        out.push('Z');
        out
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl From<std::time::SystemTime> for Timestamp {
    /// Converts a [`std::time::SystemTime`] into a [`Timestamp`], saturating to
    /// [`Timestamp::UNIX_EPOCH`] if `time` is before the Unix epoch or beyond
    /// `i64` milliseconds.
    fn from(time: std::time::SystemTime) -> Self {
        time.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis)
    }
}

/// Proleptic Gregorian `(year, month, day)` for a count of days since
/// `1970-01-01`, valid for any `i64` day count that keeps the arithmetic in
/// range. This is Howard Hinnant's `civil_from_days`: the calendar is
/// shifted so each 400-year era starts on March 1, which puts the leap day
/// last and makes every month length a closed-form expression.
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    // The day within the era, `0..=146_096`.
    let doe = z.rem_euclid(146_097);
    // The year within the era, `0..=399`.
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    // The day within the March-based year, `0..=365`.
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    // The March-based month, `0..=11`.
    let mp = (5 * doy + 2) / 153;
    // Every value below is bounded by the comments above, so the narrowing
    // conversions cannot fail; `unwrap_or` keeps the function total without
    // a panic path.
    let day = u8::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u8::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
