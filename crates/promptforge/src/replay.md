Behavior flags, recorded with each run beside its seed and start instant.

A run is deterministic. Three inputs plus the host's answers, replayed in order, produce the same effects and events. The seed and the start instant are the first two inputs, and this module holds the third, [`Flags`]. Flags let a future engine change that alters a run's behavior stay replayable. A live run that uses the new behavior records its flag, and a replay honors the new behavior only if the original run recorded that flag. No flag is defined yet, so every run this engine produces records [`Flags::EMPTY`]. A host records the flags with each run today and hands them back when it rebuilds the run, so its records are complete when the first flag arrives.

# Where this fits

Flags touch the host loop only at its two ends, through the [`RunContext`](crate::RunContext). No [`Effect`](crate::effect::Effect), [`EffectAnswer`](crate::effect::EffectAnswer), or [`Event`](crate::event::Event) holds them.

1. **Building the run.** [`RunContext::new`](crate::RunContext::new) starts every context with [`Flags::EMPTY`]. The host reads the set with [`RunContext::run_flags`](crate::RunContext::run_flags) and records it next to the seed and start instant. Recording all three before [`Run::new`](crate::Run::new) means the record exists however the run ends.
2. **Driving the run.** While the host drives the run with [`Run::step`](crate::Run::step) and [`Run::resume`](crate::Run::resume), it records the [`EffectRecord`](crate::effect::EffectRecord) and [`AnswerRecord`](crate::effect::AnswerRecord) of each effect it performs. The [`effect`](crate::effect) module page covers those records.
3. **Reproducing the run.** A host rebuilds the context with the recorded seed and start instant, hands the recorded flags back through [`RunContext::flags`](crate::RunContext::flags), and replays the recorded answers in order.

Replay itself is not built yet. The crate defines what to record, but nothing re-executes a log today.

# Recording and restoring flags

This program records a live context's inputs, then rebuilds a context from the record the way a replay would.

````
use promptforge::replay::Flags;
use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

let started_millis = 951_782_400_000;
let ctx = RunContext::new("greeter", 7, Timestamp::from_unix_millis(started_millis));

let recorded_seed = ctx.seed();
let recorded_flags = ctx.run_flags().bits();
assert_eq!(recorded_flags, 0);

let replay = RunContext::new("greeter", recorded_seed, Timestamp::from_unix_millis(started_millis))
    .flags(Flags::from_bits(recorded_flags));
assert_eq!(replay.run_flags(), Flags::EMPTY);
assert_eq!(replay.seed(), 7);
assert_eq!(replay.started_at(), ctx.started_at());
````

Here is what each part does.

1. **Build the live context.** [`RunContext::new`](crate::RunContext::new) puts [`Flags::EMPTY`] on a fresh context. A live host never calls [`RunContext::flags`](crate::RunContext::flags), so it has nothing to set.
2. **Record the inputs.** [`RunContext::run_flags`](crate::RunContext::run_flags) returns the context's [`Flags`]. It is named `run_flags` because the builder method is already named `flags`. [`Flags::bits`] turns the set into one [`u32`], which fits an integer column next to the seed from [`RunContext::seed`](crate::RunContext::seed) and the start instant from [`RunContext::started_at`](crate::RunContext::started_at). The in-repo harness records the seed, flags, and start instant when it begins its run record, but it writes the literal `0` for the flags instead of reading [`RunContext::run_flags`](crate::RunContext::run_flags). Read the value from the context instead, so the records stay correct once flags exist.
3. **Restore the flags.** [`Flags::from_bits`] rebuilds the set from the stored integer, and [`RunContext::flags`](crate::RunContext::flags) sets it on the replay's context. [`RunContext::run_flags`](crate::RunContext::run_flags) then returns the recorded set.

# A pass-through input

The engine stores whatever set is passed to [`RunContext::flags`](crate::RunContext::flags) and returns it unchanged. Nothing in this build branches on the flags during a run. The set shows up in only two places: [`RunContext::run_flags`](crate::RunContext::run_flags), and the context's [`Debug`](std::fmt::Debug) output, which prints the flags between the seed and the start instant. So a non-empty set changes no behavior today. The context simply keeps it:

````
use promptforge::replay::Flags;
use promptforge::timestamp::Timestamp;
use promptforge::RunContext;

let recorded = Flags::from_bits(0b101);
let ctx = RunContext::new("greeter", 42, Timestamp::UNIX_EPOCH).flags(recorded);
assert_eq!(ctx.run_flags(), recorded);
assert_eq!(ctx.seed(), 42);
````

# Storing a flag set

A flag set is one [`u32`] on the wire and in the run record. [`Flags::bits`] gives the integer to store, and [`Flags::from_bits`] rebuilds the set from it. Any [`u32`] is valid. [`Flags::from_bits`] keeps every bit, including bits this build does not name, and neither rejects nor masks them. So `Flags::from_bits(0b101).bits()` is `0b101`, and a record written by a newer engine keeps its flags through an older reader.

Bit numbering never changes. Each flag that a future change introduces will be an associated constant on [`Flags`] that sets one bit `n`, written `1 << n`. Bit `n` is assigned once and never reused or renumbered, even after the behavior it gated becomes the only behavior. A stored integer therefore means the same thing to every engine version.

A host that keeps its run records as JSON can store the set directly. With serde, [`Flags`] serializes and deserializes as one bare JSON integer:

````
use promptforge::replay::Flags;

assert_eq!(serde_json::to_string(&Flags::from_bits(6))?, "6");
assert_eq!(serde_json::from_str::<Flags>("6")?, Flags::from_bits(6));
assert_eq!(serde_json::to_string(&Flags::EMPTY)?, "0");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Testing and combining flags

Whether a future replay honors a behavior depends on a test of the recorded set, and [`Flags::contains`] is that test. It returns `true` when every flag in its argument is set in the recorded set, and the empty set is contained in every set. [`Flags::is_empty`] tells whether a run recorded any flag at all. [`Flags::EMPTY`] and [`Flags::default`] are the same empty set, with bits `0`, so either works in a comparison. Sets combine with the `|` operator, which gives their union, and with `|=`. Those are the only set operators, so test membership with [`Flags::contains`].

````
use promptforge::replay::Flags;

let recorded = Flags::from_bits(0b101);
assert!(recorded.contains(Flags::from_bits(0b100)));
assert!(!recorded.contains(Flags::from_bits(0b010)));
assert!(recorded.contains(Flags::EMPTY));
assert!(!recorded.is_empty());

let mut combined = Flags::from_bits(0b001) | Flags::from_bits(0b100);
assert_eq!(combined, recorded);
combined |= Flags::from_bits(0b010);
assert_eq!(combined.bits(), 0b111);

assert_eq!(Flags::default(), Flags::EMPTY);
assert!(Flags::EMPTY.is_empty());
````

[`Flags::from_bits`], [`Flags::bits`], [`Flags::is_empty`], and [`Flags::contains`] are all `const fn`, and [`Flags::EMPTY`] is an associated constant. So a host can build and test flag sets in `const` items:

````
use promptforge::replay::Flags;

const RECORDED: Flags = Flags::from_bits(0b101);
const HAS_BIT_TWO: bool = RECORDED.contains(Flags::from_bits(0b100));
assert!(HAS_BIT_TWO);
assert_eq!(RECORDED.bits(), 5);
````

# Reference

This module holds one item, the [`Flags`] struct. The two methods that set and read a run's flags live at the crate root: [`RunContext::flags`](crate::RunContext::flags) and [`RunContext::run_flags`](crate::RunContext::run_flags).

## Flags

[`Flags`] is the set of behavior flags recorded with a run, a bitset that is one [`u32`] on the wire and in the run record. It is `#[repr(transparent)]` over [`u32`]. No named flag constants exist yet.

A host reads a context's set with [`RunContext::run_flags`](crate::RunContext::run_flags). It uses [`Flags::EMPTY`] or [`Flags::default`] for the empty set. It rebuilds a stored set with [`Flags::from_bits`], or deserializes one from a JSON integer. And it combines existing sets with `|`. [`Flags`] is [`Copy`], so every method takes `self` by value.

- [`Flags::EMPTY`] is the associated constant for the set with no flag set, with bits `0`. It equals [`Flags::default`]. [`RunContext::new`](crate::RunContext::new) puts it on every fresh context.
- [`Flags::from_bits`] takes `bits`, a [`u32`], and returns the [`Flags`] whose bits are exactly `bits`. Pass the integer the host stored from a recorded run's [`Flags::bits`]. Any [`u32`] is valid, and bits this build does not name are kept. It cannot fail. A replay host passes the result to [`RunContext::flags`](crate::RunContext::flags).
- [`Flags::bits`] takes no arguments and returns the set as its [`u32`] bits, unknown bits included. The host stores this in its run record next to the seed and start instant. It cannot fail. `Flags::EMPTY.bits()` is `0`.
- [`Flags::is_empty`] takes no arguments and returns a [`bool`]: `true` when no flag is set, meaning the bits are `0`, and `false` otherwise. It cannot fail. `Flags::from_bits(0b101).is_empty()` is `false`.
- [`Flags::contains`] takes `other`, a [`Flags`] holding the flag or flags to test for, and returns a [`bool`]. It is `true` when every flag in `other` is set in `self`. Any set is a valid argument, and [`Flags::EMPTY`] is contained in every set. It cannot fail. This is the check that lets a replay honor a behavior only if the original run recorded its flag.

All four methods are `const fn` and `#[must_use]`, so discarding a result is a compiler warning.

The caller-relevant trait impls:

- [`Default`]: [`Flags::default`] is [`Flags::EMPTY`], with bits `0`.
- Serde: [`Flags`] serializes and deserializes as one bare integer. `Flags::from_bits(6)` serializes as `6`, and [`Flags::EMPTY`] serializes as `0`.
- [`BitOr`](std::ops::BitOr) and [`BitOrAssign`](std::ops::BitOrAssign): `a | b` is the union of two sets, and `a |= b` adds the flags of `b` to `a`.

[`Flags`] has no [`FromStr`](std::str::FromStr), [`Display`](std::fmt::Display), or [`From`] impl. It converts only through [`Flags::from_bits`], [`Flags::bits`], and serde.


