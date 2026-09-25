The start instant of a run, built from signed Unix milliseconds and rendered as RFC 3339.

A run never reads a clock, so the host tells it when it started. This module holds the one type for that job, [`Timestamp`], a UTC instant stored as signed milliseconds since the Unix epoch, `1970-01-01T00:00:00Z`. The host builds one from its own clock, from a recorded value, or from a fixed constant, and passes it to [`RunContext::new`](crate::RunContext::new). Every section of the prompt then reads that instant as `sys.when`. Because the host picks the value, a test can pin it, and a replay can hand back exactly the instant the original run saw. By the end of this page you can stamp a run from the system clock, pin a start instant for tests, predict what the prompt reads, and record the value for replay.

# Where this fits

A [`Timestamp`] enters the host loop once, before the first step. The host passes it as the `started_at` argument of [`RunContext::new`](crate::RunContext::new), and then builds the [`Run`](crate::Run) that it drives with [`Run::step`](crate::Run::step) and [`Run::resume`](crate::Run::resume). [`RunContext::new`](crate::RunContext::new) has no default start instant, so every host supplies one.

The run does not read the time during the loop, so the time never travels in an [`Effect`](crate::effect::Effect) or an [`EffectAnswer`](crate::effect::EffectAnswer). Instead, the run renders the start instant once with [`Timestamp::to_rfc3339`], and the H1 pass and every section see that same string as `sys.when`. [`RunContext::started_at`](crate::RunContext::started_at) returns the value unchanged, so the host can record it with the seed and replay the run later.

# Stamping a run from the system clock

A live host stamps each run with the current time. This program reads the system clock through the standard library and builds the run's context from it:

````
use std::time::{SystemTime, UNIX_EPOCH};

use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

let started_at = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .ok()
    .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
    .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis);

let ctx = RunContext::new("greeter", 7, started_at);
assert_eq!(ctx.started_at(), started_at);
assert!(Timestamp::UNIX_EPOCH < started_at);
````

Here is what each part does.

1. **Measure from the epoch.** [`SystemTime::now`](std::time::SystemTime::now) reads the clock, and [`SystemTime::duration_since`](std::time::SystemTime::duration_since) with [`std::time::UNIX_EPOCH`] gives the time elapsed since 1970 as a [`Duration`](std::time::Duration). It fails when the clock reads earlier than the epoch, and [`Result::ok`] turns that failure into [`None`].
2. **Narrow to milliseconds.** [`Duration::as_millis`](std::time::Duration::as_millis) returns a [`u128`], and [`i64::try_from`](std::convert::TryFrom::try_from) narrows it to the [`i64`] that [`Timestamp::from_unix_millis`] takes. The narrowing fails only for a clock past the [`i64`] millisecond range. Precision below one millisecond is dropped here, because a [`Timestamp`] counts whole milliseconds.
3. **Build the timestamp, with a fallback.** [`Option::map_or`](std::option::Option::map_or) passes the count to [`Timestamp::from_unix_millis`], or uses [`Timestamp::UNIX_EPOCH`] when either earlier step failed. The repository's own harness host uses this exact chain, so a clock before the epoch or past the [`i64`] range starts the run at the epoch instead of refusing to launch it.
4. **Pass it to the context.** [`RunContext::new`](crate::RunContext::new) takes the run's name, a seed, and the start instant. [`RunContext::started_at`](crate::RunContext::started_at) returns the same [`Timestamp`], unchanged.

The start instant and the seed are independent inputs. Changing the seed changes the nonce of the untrusted envelope but leaves `sys.when` alone, and changing the start instant leaves the nonce alone.

# Fixed start instants

Tests and reproducible runs need the same start instant every time. [`Timestamp::UNIX_EPOCH`] is the simplest choice. It is `1970-01-01T00:00:00Z`, with a millisecond count of `0`, and it suits any run that does not care about `sys.when`.

For a specific instant, [`Timestamp::from_unix_millis`] takes any signed millisecond count. It is a `const fn`, so a fixed instant can be a compile-time constant that every test shares:

````
use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

const STARTED_AT: Timestamp = Timestamp::from_unix_millis(951_782_400_123);

let epoch_run = RunContext::new("unit-test", 1, Timestamp::UNIX_EPOCH);
assert_eq!(epoch_run.started_at().unix_millis(), 0);

let pinned_run = RunContext::new("unit-test", 1, STARTED_AT);
assert_eq!(pinned_run.started_at().to_rfc3339(), "2000-02-29T00:00:00.123Z");
````

# What the prompt reads

`sys.when` is exactly the string that [`Timestamp::to_rfc3339`] returns for the start instant, so a host can compute it ahead of the run. It is not a live clock, and there is no `sys.now`. A prompt that reads `sys.now` fails its section with "unknown sys field 'now'".

The rendering is an RFC 3339 UTC string of the form `YYYY-MM-DDTHH:MM:SS[.fff]Z`. It follows three rules.

- **Fractions are trimmed.** Trailing zeros are dropped from the milliseconds, and a whole second has no fraction at all. The string always ends in `Z`.
- **The calendar is proleptic Gregorian.** Leap years are correct, so 1900 is not a leap year, 2000 is, and 2100 is not.
- **Only years 0000 through 9999 are valid.** A year outside that range renders with more digits or a sign, and the result is not RFC 3339. [`Timestamp::from_unix_millis`] accepts such counts anyway, so avoid stamping a run there.

The rendering matches the `time` crate's RFC 3339 format byte for byte. That was checked on a table of samples and on a sweep of instants from 1770 to 2170. The rendering is built on the standard library alone, with no clock or calendar dependency.

````
use promptforge::timestamp::Timestamp;

assert_eq!(Timestamp::from_unix_millis(1_709_210_096_789).to_rfc3339(), "2024-02-29T12:34:56.789Z");
assert_eq!(Timestamp::from_unix_millis(1_709_210_096_780).to_rfc3339(), "2024-02-29T12:34:56.78Z");
assert_eq!(Timestamp::from_unix_millis(1_709_210_096_700).to_rfc3339(), "2024-02-29T12:34:56.7Z");
assert_eq!(Timestamp::from_unix_millis(4_107_542_399_000).to_rfc3339(), "2100-02-28T23:59:59Z");

assert_eq!(Timestamp::from_unix_millis(-1).to_rfc3339(), "1969-12-31T23:59:59.999Z");
assert_eq!(Timestamp::from_unix_millis(-62_135_596_800_000).to_rfc3339(), "0001-01-01T00:00:00Z");
assert_eq!(Timestamp::from_unix_millis(253_402_300_799_999).to_rfc3339(), "9999-12-31T23:59:59.999Z");

let stamp = Timestamp::from_unix_millis(951_782_400_000);
assert_eq!(stamp.to_string(), stamp.to_rfc3339());
````

The last line shows the [`Display`](std::fmt::Display) impl, which writes the same string, so [`ToString::to_string`] and [`format!`] give the `sys.when` text too.

# Recording and replaying the start instant

A run is reproducible from its seed, its start instant, and its flags, plus the answers the host gave it. With the same inputs and the same answers, a run reproduces its nonces, `sys.when`, its effects, and its events. So a host that wants to replay a run records the start instant next to the seed. Replay itself is not built yet, and no behavior flags are defined yet. The [`replay`](crate::replay) module page covers the flags. Recording the count today keeps a host's logs complete for when replay arrives.

[`Timestamp::unix_millis`] returns the raw millisecond count, exactly the value given to [`Timestamp::from_unix_millis`]. Store that [`i64`] in your run log. To replay, read it back and pass it through [`Timestamp::from_unix_millis`] again, and `sys.when` comes out identical. The repository's harness host writes the count into its run log row as `started_at`, an [`i64`] of UTC milliseconds since the Unix epoch, so the log and the run agree.

````
use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

let live = RunContext::new("greeter", 7, Timestamp::from_unix_millis(951_782_400_000));
let recorded_seed = live.seed();
let recorded_started_at: i64 = live.started_at().unix_millis();

let replay = RunContext::new("greeter", recorded_seed, Timestamp::from_unix_millis(recorded_started_at));
assert_eq!(replay.started_at(), live.started_at());
assert_eq!(replay.started_at().to_rfc3339(), "2000-02-29T00:00:00Z");
````

A host that keeps its log as JSON can store the [`Timestamp`] itself. With serde it serializes and deserializes as a bare integer of Unix milliseconds, the same integer [`Timestamp::unix_millis`] returns:

````
use promptforge::timestamp::Timestamp;

let stamp = Timestamp::from_unix_millis(1_709_210_096_789);
assert_eq!(serde_json::to_string(&stamp)?, "1709210096789");
assert_eq!(serde_json::from_str::<Timestamp>("1709210096789")?, stamp);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Comparing and sharing timestamps

[`Timestamp`] implements [`Ord`] and [`PartialOrd`] in chronological order, so timestamps compare with `<` and sort from earliest to latest. It is a [`Copy`] value that holds no clock handle. The same value can go to several contexts, or across threads, and each copy is the same instant.

````
use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

let first = Timestamp::from_unix_millis(951_782_400_000);
let second = Timestamp::from_unix_millis(1_709_210_096_789);
let mut stamps = vec![second, Timestamp::UNIX_EPOCH, first];
stamps.sort();
assert_eq!(stamps, [Timestamp::UNIX_EPOCH, first, second]);

let a = RunContext::new("run-a", 1, first);
let b = RunContext::new("run-b", 2, first);
assert_eq!(a.started_at(), b.started_at());
````

# Reference

This module holds one item, the [`Timestamp`] struct. The two functions that take and return a run's start instant live at the crate root: [`RunContext::new`](crate::RunContext::new) and [`RunContext::started_at`](crate::RunContext::started_at).

## Timestamp

[`Timestamp`] is a UTC instant stored as signed milliseconds since the Unix epoch, `1970-01-01T00:00:00Z`. It is the start instant of a run, which every section of the prompt reads as `sys.when`.

A host gets one in four ways. It calls [`Timestamp::from_unix_millis`] for any instant, uses [`Timestamp::UNIX_EPOCH`] or the [`Default`] value for the epoch, or deserializes one from a JSON integer with serde. The millisecond field is private, and there is no [`From`], [`Into`], or [`FromStr`](std::str::FromStr) conversion, so those four are the only ways.

- [`Timestamp::UNIX_EPOCH`] is the associated constant for `1970-01-01T00:00:00Z`. Its [`Timestamp::unix_millis`] is `0`, and its [`Timestamp::to_rfc3339`] is `"1970-01-01T00:00:00Z"`. Use it as a fixed, deterministic start instant when nothing depends on `sys.when`, and as the fallback when the system clock cannot be converted.
- [`Timestamp::from_unix_millis`] takes `millis`, an [`i64`], and returns the [`Timestamp`] for the instant `millis` milliseconds after the Unix epoch, or before it when negative. Fill it from your clock, as [Stamping a run from the system clock](#stamping-a-run-from-the-system-clock) shows, or from a recorded count when replaying. The unit is milliseconds, not seconds or nanoseconds. Every [`i64`] is accepted with no validation or clamping, but only years 0000 through 9999 render as valid RFC 3339. It cannot fail. It is a `const fn`, so it can initialize a `const` item, as [Fixed start instants](#fixed-start-instants) shows.
- [`Timestamp::unix_millis`] takes no arguments and returns the signed millisecond count as an [`i64`], exactly the value given to [`Timestamp::from_unix_millis`]. Store it in your run log so a replay can hand it back. It cannot fail, and it is a `const fn`.
- [`Timestamp::to_rfc3339`] takes no arguments and returns the instant as a [`String`] in RFC 3339 UTC form, for example `"2024-02-29T12:34:56.789Z"`. This is exactly the string a prompt reads as `sys.when` for a run started at this instant. [What the prompt reads](#what-the-prompt-reads) gives the fraction, calendar, and year-range rules. It cannot fail.

[`Timestamp::unix_millis`] and [`Timestamp::to_rfc3339`] take `self` by value, and [`Timestamp`] is [`Copy`], so the value stays usable after each call. All three functions are `#[must_use]`, so discarding a result is a compiler warning.

The caller-relevant trait impls:

- [`Display`](std::fmt::Display) writes the same string as [`Timestamp::to_rfc3339`].
- [`Default`] is the epoch, equal to [`Timestamp::UNIX_EPOCH`].
- [`Ord`] and [`PartialOrd`] order timestamps chronologically.
- Serde serializes and deserializes a [`Timestamp`] as a bare integer of Unix milliseconds, so `1_709_210_096_789` is written as `1709210096789`.
