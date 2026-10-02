[`Timestamp`] is the start time your program gives each run, which the prompt reads as `sys.when`.

You need this when you start runs, or when you store and compare their start times.

# Where this fits

You already build a context and drive a run in [Run a prompt](crate#run-a-prompt). [`RunContext::new`](crate::RunContext::new) takes the run's start time as its third argument, and that argument is a [`Timestamp`]. This page shows how to draw that time from your clock, what the prompt sees, and how to keep the time with your own record of the run.

# Stamp a run's start

Your program starts runs, and each run needs to know when it started. The prompt reads that time, and your own record of the run keeps it.

A run reads no clock of its own. Its start time is an input, like its arguments: you draw it once from your clock, the run reads that value, and your record stores it. The value is a [`Timestamp`]. Passing the recorded value into a new run gives the same time, which a replay needs: it runs the same prompt again with the recorded inputs so that it behaves the same way.

````
# use std::sync::Arc;
use std::time::SystemTime;

# use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
# fn answer(effect: Effect) -> EffectAnswer {
#     match effect {
#         Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#         _ => EffectAnswer::Dropped,
#     }
# }

// 1. The greeter writes the start time it reads as `sys.when` into its note, and returns the note.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes its start time to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', sys.when)\n",
    "return store.read('note.md')\n",
    "```\n",
);
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = Arc::new(parsed?);

// 2. Draw the start time once from the system clock, as whole milliseconds since the epoch.
let elapsed = SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
let stamp = Timestamp::from_unix_millis(i64::try_from(elapsed.as_millis())?);

// 3. Pass the stamp as the third argument of `RunContext::new`, and drive the run as before.
let mut run = Run::new(prompt, "", RunContext::new("greeter", 7, stamp));
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 run.resume(id, answer(effect));
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };

// 4. Print the stamp with `{}` to see what the prompt read, and check that its millisecond count round-trips.
println!("the run started at {stamp}");
assert!(matches!(result, RunResult::Ok(text) if text == stamp.to_rfc3339()));
assert_eq!(Timestamp::from_unix_millis(stamp.unix_millis()), stamp);
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Only the greeter's Lua changes. It writes `sys.when` to `note.md` and returns what it reads back, so the run's result is exactly the start time the prompt saw.
2. This step is the whole clock recipe, and you write it yourself, because the crate offers no conversion from [`SystemTime`](std::time::SystemTime). Take the time elapsed since the standard library's epoch, turn its milliseconds into an `i64` with [`i64::try_from`](TryFrom::try_from), and pass that to [`Timestamp::from_unix_millis`]. The example writes [`std::time::UNIX_EPOCH`] in full only to show which epoch it means. After `use std::time::UNIX_EPOCH`, a bare `UNIX_EPOCH` is that `SystemTime`, never the associated constant [`Timestamp::UNIX_EPOCH`], which `use` cannot import.
3. Pass the stamp as the third argument of [`RunContext::new`](crate::RunContext::new). `"greeter"` is the run's name, stamped on every event it reports. `7` is the seed, which makes the nonce that wraps untrusted text, so a live program draws it from a secure random source. Like the start time, your program stores the seed to reproduce the run. The hidden lines drive the run as in [Run a prompt](crate#run-a-prompt), and [`Step::Done`](crate::Step::Done) hands back `result`, the run's [`RunResult`](crate::RunResult).
4. `{stamp}` prints the [`to_rfc3339`](Timestamp::to_rfc3339) string, such as `2000-02-29T00:00:00Z`. The first assertion proves that this string is exactly what the prompt read as `sys.when`. `{stamp:?}` shows the derived debug form, such as `Timestamp(951782400000)`, and [`unix_millis`](Timestamp::unix_millis) gives the bare integer. The second assertion shows that `unix_millis` round-trips exactly through `from_unix_millis`, so a stored millisecond count rebuilds the same stamp.

What if you pass seconds where the start time expects milliseconds? `from_unix_millis` accepts every `i64` without checking it, so the run does not fail. It starts in January 1970 instead. For example, `1_700_000_000` makes the prompt read `1970-01-20T16:13:20Z`. A `sys.when` in 1970 means you passed seconds, so multiply by 1000 or use the duration's milliseconds.

The string's fraction of a second varies in width. It is left out when the milliseconds are zero, and otherwise it loses trailing zeros, so 780 ms renders as `.78`. Code that parses the string by fixed position or sorts the strings gets wrong answers. Compare `Timestamp` values instead, because they order chronologically.

To keep the start time with your own record, store `unix_millis()` and rebuild it with `from_unix_millis`. Serde also writes a `Timestamp` as that bare integer, not as a string. It reads any `i64` back without a range check, so a record written in seconds loads without any error.

For tests that never check `sys.when`, pass `Timestamp::UNIX_EPOCH`, which is the same instant as [`Timestamp::default()`](Timestamp::default). A fixed start keeps test runs deterministic without a clock.

You might expect a run to read the system clock when it starts, the way most code calls [`SystemTime::now()`](std::time::SystemTime::now). Instead, it renders the start time you pass to `RunContext::new` as `sys.when`, and writes no log. To reproduce a run, your program passes the stored start time in again, so a rerun shows the original run's time rather than the time of the rerun.

Draw the time once, in milliseconds, and hand it to the run. Next, return to [Where to go next](crate#where-to-go-next) to pick your next page.

# Reference

## Timestamp

[`Timestamp`] holds the UTC instant a run started, as signed whole milliseconds since the Unix epoch, negative before 1970. Pass it as the third argument of [`RunContext::new`](crate::RunContext::new), and a prompt reads its RFC 3339 rendering as `sys.when`. Nothing checks the value, so a count in seconds lands silently in January 1970. Store its millisecond count with your record, compare timestamps directly because they order chronologically, and render one only to show it. [Stamp a run's start](#stamp-a-runs-start) teaches it.

- [`UNIX_EPOCH`](Timestamp::UNIX_EPOCH): the zero instant, `1970-01-01T00:00:00Z`, equal to [`Timestamp::default()`](Timestamp::default). It is not [`std::time::UNIX_EPOCH`], which is a `SystemTime`.
- [`from_unix_millis`](Timestamp::from_unix_millis): takes milliseconds, not seconds, and accepts every `i64`. Negative values precede the epoch and round earlier, so -1 ms renders `1969-12-31T23:59:59.999Z`.
- [`unix_millis`](Timestamp::unix_millis): returns the millisecond count, which round-trips exactly through `from_unix_millis`.
- [`to_rfc3339`](Timestamp::to_rfc3339): renders UTC ending in `Z`, such as `2024-02-29T12:34:56.789Z`, exactly as `sys.when`. Trailing fraction zeros drop, and years outside 0000 to 9999 break RFC 3339.
- Display and serde: `{}` writes exactly `to_rfc3339`. Serde writes the bare millisecond integer and reads any `i64` back without a range check.
