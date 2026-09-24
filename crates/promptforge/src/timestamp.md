The UTC instant a run starts from.

# The host's clock

The engine reads no clock. A run's start instant is an input the host draws and passes to [`RunContext::new`](crate::RunContext::new); a host that records a run keeps it in the run log, and a replay hands the recorded value back verbatim. [`Timestamp`] is the value that crosses that boundary: signed milliseconds since the Unix epoch.

# What a prompt reads

Every section, the H1 pass included, reads the start instant as `sys.when`, in the one rendering [`Timestamp::to_rfc3339`] produces. It is written over the standard library alone, so the engine takes no clock or calendar dependency, and it agrees byte for byte with the `time` crate's RFC 3339 rendering of the same instant.

```
use promptforge::timestamp::Timestamp;

let leap_day = Timestamp::from_unix_millis(951_782_400_000);
assert_eq!(leap_day.to_rfc3339(), "2000-02-29T00:00:00Z");
assert_eq!(Timestamp::UNIX_EPOCH.unix_millis(), 0);
```
