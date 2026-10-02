[`Flags`] is the set of behavior bits a [run](crate) records, which you store with its record and hand back unchanged.

You need this page when your program keeps a record of each run and restores that record later.

# Where this fits

From [the crate overview](crate), you know how to build a [`RunContext`](crate::RunContext) and step a run to its result.

A run also carries a small set of behavior bits. To replay a run, your program runs the prompt again from its record. It builds a new `RunContext` with the recorded seed, start time, and flags, and answers each effect with the answer the record holds for it, in the same order. The same inputs and the same answers produce the same effects and events.

When a later version of this crate changes how a recorded run behaves, it gives that change its own bit, so a record that carries the bit tells that replay to reproduce that behavior. These bits are the run's *flags*. This crate names no flag, so no bit changes how a run behaves, and your program only stores and returns the set.

You give a run its flags through [`RunContext::flags`](crate::RunContext::flags) and read them through [`RunContext::run_flags`](crate::RunContext::run_flags). Like the seed, the `u64` you pass to [`RunContext::new`](crate::RunContext::new) and read back with [`RunContext::seed`](crate::RunContext::seed), the flags are a run input your program records and hands back. Unlike the seed, a fresh context starts them empty, and you set them with the `flags` builder. This page shows what to store with each run record, and how to hand it back.

# Keep a run's flags

Your program must save a run's flags with its record, and restore them exactly when it loads the record later.

[`Flags`] feels like a [`bitflags`](https://docs.rs/bitflags) type: a `u32` of bits you combine with `|` and test with `contains`. Unlike a `bitflags` type, it keeps bits it does not name, and it names no flag at all. So treat the flags as data. Your program stores them and returns them untouched, and it never needs to know what a bit means.

````
use promptforge::replay::Flags;
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = Arc::new(parsed.expect("the greeter parses"));
# fn run_to_end(mut run: Run) -> RunResult {
#     loop {
#         match run.step() {
#             Step::Pending { effects, .. } => {
#                 for (id, _provenance, effect) in effects {
#                     let answer = match effect {
#                         Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#                         _ => EffectAnswer::Dropped,
#                     };
#                     run.resume(id, answer);
#                 }
#             }
#             Step::Done { result, .. } => return result,
#         }
#     }
# }

// 1. Run the greeter, and save its flags with its record as one number.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
let original = ctx.run_flags();
let stored: u32 = original.bits();
let first = run_to_end(Run::new(Arc::clone(&prompt), "", ctx));
assert!(matches!(first, RunResult::Ok(text) if text == "hello"));
assert_eq!(stored, 0);

// 2. Rebuild the flags from the stored number, and give them to the next run.
let restored = Flags::from_bits(stored);
assert_eq!(restored, original);
let next = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).flags(restored);
assert_eq!(next.run_flags(), original);
let second = run_to_end(Run::new(prompt, "", next));
assert!(matches!(second, RunResult::Ok(text) if text == "hello"));

// 3. Round-trip the number a newer version's record holds, with bits this version does not name.
let newer = Flags::from_bits(0b101);
assert_eq!(newer.bits(), 0b101);
assert!(newer.contains(Flags::from_bits(0b100)));
assert!(!newer.contains(Flags::from_bits(0b010)));
````

1. Step 1 reads the flags with [`RunContext::run_flags`](crate::RunContext::run_flags) before [`Run::new`](crate::Run::new) takes the context. A fresh context holds [`Flags::EMPTY`], the same value as [`Flags::default()`](Flags::default), because this crate names no flag. [`Flags::bits`] turns the set into a plain `u32`, so the stored number is `0`, and an ordinary integer column in your record holds it. The run itself goes through `run_to_end`, the store-only loop from the crate overview, hidden above, and returns `hello`.
2. Step 2 rebuilds the set from the stored number with [`Flags::from_bits`], and the result equals the original. For every `x`, `Flags::from_bits(x).bits() == x`, so the stored number restores the exact set. [`RunContext::flags`](crate::RunContext::flags) gives the set to the next run's context, which reports it back unchanged, and the next run returns `hello` again.
3. Step 3 rebuilds `0b101`, a number that a record from a newer version could hold. `from_bits` keeps every bit, including bits this version does not name, so `bits` returns `0b101` unchanged. An older program that loads and saves a newer record never loses its flags. The step then tests single bits with [`Flags::contains`], which is true only when every bit of its argument is set in the set. The set holds bit `0b100`, so that test is true, and it does not hold `0b010`, so that test is false.

`contains` is a subset test, not an overlap test, so test one flag at a time. Every set contains `Flags::EMPTY`, so never use the empty set to ask whether a flag is present. To ask whether a record carries any flag at all, known or not, call [`Flags::is_empty`]. It is true only when the number is `0`, so a set that holds only bits this version does not name is not empty.

With [serde](https://docs.rs/serde), `Flags` serializes as a bare number, not as a struct or a list of names. `Flags::from_bits(6)`, the set holding bits `0b010` and `0b100`, serializes as `6`, and deserializing `6` gives the same set back. The empty set a fresh run records serializes as `0`.

You build a value only from `Flags::EMPTY`, `Flags::default()`, `from_bits`, `|`, and `|=`, or by deserializing a stored number, which accepts any `u32` just like `from_bits`.

There is no intersection, removal, or negation. Combine sets with `|`, and to drop or mask a bit, do the bit work on the number from `bits()` and call `from_bits` on the result.

Each flag that a later version adds owns one bit for good, and a later version never reuses or renumbers a bit. A number you store keeps its meaning in every later version, so you never migrate old records. That is why a run carries a flag set even though this crate names no flag: your records hold the column from the first run.

You might expect `Flags::from_bits` to act like the `bitflags` crate's and reject bits it does not know. Instead, it accepts every `u32` and keeps every bit, so an unknown bit survives the trip through your record.

Store the number, and hand it back unchanged. The [Reference](#reference) entry below sums up each member of `Flags`.

# Reference

## Flags

[`Flags`] holds the behavior flags recorded with a run, carried as one `u32` on the wire and in the run record. Use it when you keep run records: store its number with each run, and rebuild the set from that number when you read the record back. Nothing about it fails. Only `|` and `|=` combine sets, so rebuild from the number to drop a flag. [Keep a run's flags](#keep-a-runs-flags) teaches it.

- [`EMPTY`](Flags::EMPTY): the set with no bit set, equal to [`Flags::default()`](Flags::default), and the only constant, because this crate names no flag.
- [`from_bits`](Flags::from_bits): accepts any `u32` and keeps every bit; unlike the `bitflags` crate's `from_bits`, it never rejects unknown bits.
- [`bits`](Flags::bits): returns the raw `u32`, unknown bits included, the exact inverse of `from_bits`.
- [`is_empty`](Flags::is_empty): true only when the raw value is `0`, so a set holding only unnamed bits is not empty.
- [`contains`](Flags::contains): a subset test, true when every bit of `other` is set; `0b101` does not contain `0b110`, and every set contains `EMPTY`.
