# Bounded store reads, a global `untrusted`, and a linear section VM startup

## Executive summary

PromptForge's Lua surface gains a global `untrusted(s)` that guard-wraps any string for model-facing injection, and the run-scoped store gains bounded reads: `store.read(path[, start[, end]])` returns a verbatim 1-based inclusive line slice, and `store.read_numbered(path[, start[, end]])` returns the same slice with absolute line numbers. The two fused methods they replace - `store.inject` and `store.read_lines` - are removed; indexing either yields nil and calling it raises. Section VM startup becomes one linear sequence: build the hardened VM, apply the run's limits, inject the host values, install the host APIs and the control globals, replay the shared library as the section's first chunk with the full environment in place, install the captured alias globals, then run the section's blocks. The reorder fixes a latent bug in passing: the shared replay previously ran under default resource limits rather than the configured ones. The one prompt-facing break is accepted without moving the engine version - frontmatter stays `promptforge: 1` - because every other change is additive, and the broken call fails loudly instead of silently changing behavior.

Each change removes a fork: one way to wrap instead of two, one bounds convention instead of two, one startup path instead of two phases.

## Key design choices

1. **One global wraps untrusted text.** `untrusted(s)` is a pure string-to-string function installed at VM construction, so it is available in every phase the engine runs: H1 blocks, section chunks, fanout arms, the shared-library replay, and local tool handlers. Making it a global rather than a store method keeps wrapping orthogonal to storage - the tool loop already wrapped untrusted tool output directly, and a store-bound inject forced a write-then-inject round trip for any string not already in a file. The name is prompt-facing vocabulary; every prompt that guards text calls it by name, so renaming later means editing every prompt. That cost is accepted because `untrusted` says exactly what the envelope asserts about the content.

2. **The guard envelope is single-use and self-contained.** Each call mints a fresh 128-bit nonce rendered as 32 hex digits and returns a preface sentence that names the tag without angle brackets, then an open tag `<untrusted_input_{nonce}>`, then the content with every literal `<` escaped to `&lt;`, then the matching close tag. Escaping every `<` - not just forged guard tags - means no content-supplied markup survives as a live delimiter, so the block is always balanced and the closing delimiter is unguessable. The envelope is defense in depth, not a security boundary: it raises the cost of an accidental or opportunistic instruction break-out, and its own documentation says so. Two calls on the same input never produce the same output, because a nonce is never reused.

3. **Composition replaces `store.inject`.** The fused read-and-wrap method is removed; `untrusted(store.read(path))` is the one way to put stored content in front of a model. A fused method duplicated `read` under a second name and hid the wrap inside a store call, and two ways to wrap invite the question of which one is correct. This is the rework's one prompt-facing break, and it is accepted as one: frontmatter stays `promptforge: 1` because everything else is additive, and a prompt that still calls `store.inject` gets nil on indexing and an error on calling - loud failure, not silent drift.

4. **Reads take 1-based inclusive bounds.** `store.read(path)` returns the whole file verbatim, `store.read(path, start)` reads from `start` to end of file, and `store.read(path, start, end)` returns lines `start` through `end` joined by newlines with no trailing newline. Bounds resolve in one fixed order: a `start` below 1 is an error; a `start` past the last line reads as the empty string; an omitted `end` means the last line and a given `end` clamps down to it; an `end` before `start` at that point is an error; an `end` without a `start` is refused rather than silently ignored. Inclusive start-and-end matches how authors and models already talk about line ranges - extraction schemas speak in `start_line`/`end_line`, and citations say "lines 22-82". The rejected alternative, `(start, count)`, forces `end - start + 1` arithmetic at every call site; with numbered text, start and end are extractive copies of what the text already shows, while count is always computed - a documented small-model failure mode.

5. **Numbering stays in the store as `read_numbered`.** `store.read_numbered(path[, start[, end]])` takes the same bounds and prefixes each line with its absolute number, right-aligned to the width of the largest emitted number, followed by `"| "`; with no bounds the whole file is numbered from 1. A bounded slice keeps its absolute numbers, so a line cited by number can be verified against the whole-file numbering. Numbering lives in the store because numbering a range needs the range's origin, and the store is the component that knows that provenance at slice time. The rejected alternative, a pure `numbered(text, start)` global, forced either repeating the start literal at every call site or reading a whole file to slice a few hundred lines. It can still be added later if numbering non-store strings shows up in practice.

6. **`store.read_lines` is gone, and the backend trait never slices.** `read_numbered` with no bounds reproduces the removed method's output byte for byte, so no capability is lost. The `Store` backend contract keeps only whole-file `read`; slicing and numbering are derived once, above the trait, so a new backend implements storage and never re-implements line mechanics. That is the structural half of the change: the surface a backend must satisfy shrank while the surface a prompt can use grew.

7. **Section startup is one linear sequence.** The two-phase construction - in which the shared library replayed before host APIs existed and limits landed afterward - is replaced by a single fixed order that every section VM drives: walked sections, `execute()` subroutines, and fanout arms all take the same path. One startup path means one ordering to document, one to test, and one for prompt authors to form a model of. The sequence itself is a design contract and appears below.

8. **The replay spends the run's configured limits.** Limits apply before the shared library replays, so load-time work is governed by the caller's configured memory and log budgets rather than construction defaults. This closes the latent bug in which the replay ran under default limits because limit application landed after it. A single instruction counter covers the replay and every later chunk, so work the shared library does at load visibly shrinks the budget left for the section - the accounting is honest rather than generous.

9. **The shared library replays as the section's first chunk, with the full environment.** Shared top-level code sees `args`, `sys`, `var`, `reply`, `log`, `store`, the `tools`/`models` tables, and the control globals, exactly as a section chunk does; a scalar top-level return is discarded, because the replay loads a library rather than producing the section's result. Two alternatives lost. The status quo blocked `log`, `store`, and `args` at load - the asymmetry that prompted the rework. A phase-gated environment that raised on host access during replay cost roughly 130 lines and a flag protocol with no observed confusion to justify it. Reversing the choice later is the expensive kind of change: shared libraries written against the full environment use host services at load, and gating them after the fact breaks their load-time behavior. If real prompts show confusing load-time behavior, the tightening path is a phase-aware proxy environment for the replay chunk only, deferred until evidence asks for it.

10. **`jump` during the replay is a hard error.** A recorded jump fails the load with "jump is not available during shared library load". Load-time control transfer has no section walk to transfer into, and following the jump - the rejected alternative - has no coherent meaning. The error names the phase so the author learns the rule from the failure.

11. **A declared alias wins over a same-named shared global.** The captured tool and model alias globals install after the replay, preserving the collision semantics prompts already had. During the replay the `tools`/`models` tables are fully functional, but the bare alias globals do not exist yet. Installing the aliases first would let a shared library silently shadow a capability the prompt declared - a worse failure than a nil global at load, because declarations are the prompt's contract with the host.

12. **An absent shared library is an empty chunk, not a skipped step.** The replay is unconditional: a prompt with no shared library replays an empty compiled chunk, and the startup sequence carries no optional branch. The visible consequence is uniform observation - shared-load started and succeeded events fire for every section - and the structural consequence is one path instead of two.

## Section startup is one fixed sequence

The order below is the contract; prompts and shared libraries may rely on what is visible at each step.

1. Build the hardened VM: sandbox, default resource ceilings, instruction budget, `untrusted`.
2. Apply the run's limits: memory ceiling and log budget.
3. Inject the host values: `args`, `sys`, `var`, `reply`, and the validating `tools`/`models` tables.
4. Install the host APIs: `log` and `store`, persistent for the section's whole lifecycle.
5. Install the control globals: `tasks`, `execute`, `jump`, `fanout`.
6. Replay the shared library as the section's first chunk; a `jump` here is a hard error.
7. Install the captured tool and model alias globals.
8. Run the section's blocks in order.

Every fallible step before the first block crosses the same observed teardown boundary, so a partially started section never runs a chunk.

## The public surface trades fused methods for composable ones

The Lua store table carries eight methods - `read`, `read_numbered`, `write`, `append`, `str_replace`, `delete`, `glob`, `exists` - and the global `untrusted(s)` stands beside it. The two bounded reads share one signature shape:

```lua
store.read(path[, start[, end]])
store.read_numbered(path[, start[, end]])
```

The crate's public handle exposes the same contract to Rust callers, with the omitted-`end` case explicit in the type:

```rust
pub fn read_range(&self, path: &str, start: usize, end: Option<usize>) -> Result<String, StoreError>
pub fn read_range_numbered(&self, path: &str, start: usize, end: Option<usize>) -> Result<String, StoreError>
```

Both report `StoreError::NotFound` for a missing file and `StoreError::InvalidRange` for a `start` below 1 or an `end` before `start` - one bounds rule, two renderings. Removed names are part of the contract too: `store.inject` and `store.read_lines` index to nil and raise when called, so stale prompts fail at the call site with the section's ordinary Lua error, not at a version check.

## Observation stays a fixed vocabulary

The observer detail vocabulary gains a read-numbered succeeded/failed pair beside the existing read pair, and loses the inject and read-lines pairs with the methods that reported them. Because the replay is unconditional, the shared-load started/succeeded/failed triple now fires for every section, including one whose prompt declares no shared library - an observer sees one uniform section lifecycle rather than two shapes.

## What reversing the headline choices would cost

The vocabulary words - `untrusted`, `read_numbered`, and the 1-based inclusive bounds convention - are prompt-facing, so changing any of them edits every prompt that speaks them; that is the ordinary, accepted cost of a name. The deep reversal is the full-environment replay: shared libraries will be written against load-time access to `log`, `store`, `args`, `var`, and the tables, and restricting that access later changes the behavior of libraries already in the field. The phase-aware replay gate is the only reversal contemplated, and it is deliberately deferred. The linear startup is cheap to rearrange internally precisely because it is one path - but its observable order (limits before replay, replay before aliases, `jump` excluded at load) is the contract prompts will rely on, and that order should be treated as fixed.

## Deferred until use proves them

Two additions stay out until evidence asks for them, and both are cheap to add later because the surfaces they extend are additive: a pure `numbered(text, start)` global, if numbering strings that did not come from the store shows up in practice, and a phase-aware gate on the replay environment, if load-time host access confuses real prompts.

*2026-08-18 16:05 - Kimi K3*
