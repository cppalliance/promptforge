# One engine, two thin drivers: the fanout arm is an adapter

## Executive summary

PromptForge has exactly one engine for executing a section's block lifecycle, and every way a section runs drives it: the chain walk (`run_one_section`) and the fanout arm (`run_one_arm`) are two thin adapters over the same setup half, the same block walk, and the same control-global constructor. The seam between engine and driver is one enum, `SectionFlow`, reporting how a section ended; each driver maps that ending to its own outcome vocabulary. A fanout arm is therefore normal flow with a short, closed list of genuine deltas - an owned payload across the spawn boundary, the `item` global, `sys.taskid` beside the parent's `sys.id`, result mapping, an exhaustion soft-degrade, proxied observation, a re-installed cancel handle, and a shared turn counter. Everything the arm once did differently by drift - stubbed control globals, no infer hook, no lazy client, single-prose-only, no conversation roll-forward - is deleted: arms run the full block walk with `execute`, `fanout`, `list_from_section`, and `jump` all live, and a jump from an arm drives a child walk instead of erroring. Fanout's only remaining bound is the concurrency window; the total item cap and its `RunLimits` API are gone. One fix to the engine now lands in one place for every path a prompt can take.

## Key design choices

1. **One engine; the arm is an adapter, not a peer implementation.** The engine is the section lifecycle itself - VM setup, infer hook, the ordered block walk with its conversation state and per-block scope rebuild - and both drivers call it. The engine contains zero arm special-casing: every genuine difference is adapter code at the driver boundary, never a knob inside the walk. The rejected alternative was two drivers sharing helper functions, which preserves accidental differences behind the word "driver" and leaves the next divergence nowhere visible to live. Reversing this later means re-forking the block walk, which is precisely the duplication debt this design exists to retire.

2. **The lifecycle is one fixed sequence with driver-owned ends.** Construction and the Lua limits install stay with each driver, because their failure handling genuinely differs: a limits failure propagates as a bare error before any teardown observation exists - routing it through teardown would emit events the walk never emitted - and an arm's VM must outlive its cancel-scoped body so the arm's single epilogue can tear it down. Between those ends the sequence is shared and ordered: host injection, host APIs, control globals, the shared-library replay, captured bindings, the infer hook, the block walk, then exactly one teardown at the driver's own boundary.

3. **`SectionFlow` is the only seam between engine and driver.** The block walk reports one of three endings: `FellThrough` (ran off the section end, carrying the rolled-forward reply), `Returned` (a scalar early return), `Jumped` (a heading and the reply at jump time). The chain driver continues or ends chains on these; the arm maps them to `LuaFanoutResult`. Because the seam is a value, not a callback or policy object, a new ending kind would have to name itself in both drivers' match arms - the compiler enforces the adapter's completeness.

4. **Fall-through inherits the incoming reply.** `FellThrough` carries the engine's reply roll-forward, so an arm whose worker produces no output returns the incoming reply text, not an empty string - the same semantics a walked section exhibits toward the next section. The alternative, keeping the arm's old hand-rolled empty-string behavior, would make worker authorship depend on whether the worker runs walked or mapped, which is the special-case thinking this design removes.

5. **Arms speak the full control surface, including jump.** The stubs are gone: `execute`, `fanout`, and `list_from_section` inside an arm resolve over the worker's visible set through the same constructor the walk uses, and `model:infer` works in an arm. A jump transfers control rather than erroring: the arm's remaining blocks are skipped, a child walk runs from the target under the chain-slice rule (a child target walks the worker's children; any other visible target walks the worker's home slice), and the arm's result text is that walk's returned value or final reply. Reversal cost is now high: fielded workers may call every one of these.

6. **The visible set is constructed where the layout is known, never reconstructed.** The fanout callback builds the worker's home slice - the set the worker was resolved from, minus the worker - at the moment it performs resolution, and threads it to every arm. Each arm's control globals derive their resolution set as the home slice plus the worker's children. The arm never inverts the engine's visible-set construction, so no unchecked cross-module layout invariant exists to break silently when the engine's set logic changes.

7. **Recursion accounting accumulates across the fanout boundary.** Each arm runs one execute level deeper than the fanout caller, and `MAX_EXECUTE_DEPTH` remains the single recursion constraint over `execute` and `fanout` nesting alike. The rejected alternative - resetting depth at the spawn boundary - would make fanout a recursion-cap escape hatch, an inconsistency an adversarial or careless prompt could turn into unbounded nesting.

8. **There is no item cap; concurrency is the only bound.** `RunLimits::max_fanout_items` and its getter are deleted, a public API break shipped in the same change. The concurrency window already bounds how many arms are in flight; a large collection simply takes longer, and whether that time is worth spending is the prompt author's call. The rejected "safety" cap prevented legitimate large mappings while preventing nothing the window does not already bound. A collection larger than the old default cap now succeeds by design, pinned by test.

9. **The limits vocabulary names the bound it returns.** With the item getter gone, the bare `fanout()` getter became ambiguous, so it was renamed `fanout_concurrency()` and its builder `max_fanout_concurrency()` - the `max_` builder spelling matching `max_tool_iterations` and `max_response_bytes`, since Rust inherent impls allow no same-name setter/getter pair. Naming is design here: the public surface now says which of the two former bounds every remaining name controls.

10. **One fanout shares one turn budget.** All arms of a fanout advance a single `Arc<AtomicU32>` model-turn counter rather than each arming its own. A fanout is one logical operation - the caller asked for N mapped answers, not N independent runs - so its aggregate model traffic reads and is bounded as one quantity.

11. **A stuck arm degrades; a failed arm aborts.** Tool-loop exhaustion in one arm soft-degrades to an incomplete stub that still renders its item, so one looping worker cannot kill its siblings' evidence. Any other arm error aborts the fanout and propagates. These two failure shapes are asymmetric on purpose: exhaustion is a local, expected resource event; anything else is a defect the run should not paper over.

12. **Arm observation is exactly-one-terminal over bounded side channels.** Each arm emits `FANOUT_ARM_STARTED`, then exactly one terminal event - succeeded, exhausted, failed, or cancelled - guaranteed by a finalizer whose `Drop` covers the abort paths. Arms report through bounded proxy channels that drop events on overload rather than block an arm: back-pressure can never alter execution results, only the completeness of best-effort progress reporting. The walk's direct observer calls and the arm's proxied ones are the one observability delta the spawn boundary forces.

## The arm's differences are a closed list

Everything distinguishing an arm from a walked section is enumerated here; any audit of the two drivers should find no difference off this list. This is the design's load-bearing contract - it is what makes "an arm is normal flow" checkable rather than aspirational.

| # | Delta | Walked section | Fanout arm |
|---|---|---|---|
| 1 | Inputs | Borrowed run context | Owned payload cloned across the spawn boundary, shared halves under one `Arc` |
| 2 | Seed | `initial_var` from a prior VM | `item`, the collection member, installed before the shared replay so library code may read it |
| 3 | `sys` identity | `id` = chain position | `id` = the parent section's id, plus `taskid` = 1-based collection position |
| 4 | Outcome | `SectionFlow` drives the chain | Mapped to `LuaFanoutResult`; a jump drives a child walk |
| 5 | Exhaustion | Propagates | Soft-degrades to the incomplete stub |
| 6 | Observation | `SECTION_STARTED`/`FINISHED`, direct | `FANOUT_ARM_STARTED` plus one terminal event, through bounded proxies |
| 7 | Cancellation | Inherits the task-local | Re-installs the explicit handle; a spawned task inherits nothing |
| 8 | Turn counter | The walk's own | One counter shared by all arms of the fanout |

The arm's user-visible identity (`sys.id` as the parent, `sys.taskid` as the position) is unchanged by the collapse and is depended on by existing prompts; the child walk a jump starts counts its own `sys.id` from 1, matching a contained `execute` chain.

## Failure and observation are shaped per arm, collected per fanout

The fanout scheduler owns the policy the arm cannot: results return in collection order regardless of finish order, a fatal arm error aborts outstanding siblings, and cancellation aborts all arms and drains the side channels so already-buffered events still reach the run's sinks. The finalizer's exactly-one-terminal guarantee is what lets an observer count arms without a reconciliation pass - started minus terminated is always the in-flight count, even across aborts.

## What reversing the headline choices would cost

The arm's full control surface is the expensive reversal: workers fielded since the capability flip may call `execute`, `fanout`, `list_from_section`, and `jump`, and re-gating them breaks those prompts. The `RunLimits` renames are the ordinary, accepted cost of a public API correction. Uncapping fanout is cheap to reverse mechanically and expensive socially: once authors rely on large mappings, a restored cap becomes a breaking change with no compensating safety property. The adapter structure itself is the cheapest element to keep and the costliest to lose - every future engine fix amortizes across both drivers only while the delta list stays closed.

## Deferred until use proves them

Two recorded items stand, both invisible to users and additive against the surfaces they touch: the per-section section-tree clones in the control-global captures (a performance candidate, not a correctness one), and a module split of the fanout scheduler, arm, and proxies. Neither changes anything a caller can observe, so both wait for evidence rather than speculation.

*2026-08-19 04:35 - Kimi K3*
