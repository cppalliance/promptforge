# Off-walk sections, level-independent walks, and fanout over collections

## Executive summary

PromptForge's prompt format gains a section marker, and its Lua control surface is rebuilt around it. A `---` thematic break has exactly two roles, fixed by position: as a section's first content it marks the section **off-walk** - the walk skips it, and the section runs only when addressed by `execute`, `jump`, `fanout`, or `list_from_section`; anywhere else it starts a reader-only comment region that ends at the next heading. Shared workers and shared list sections thereby live as ordinary sections that never run by default. Every section-addressing function resolves against one visible set - the caller's sibling sections minus itself, plus its direct children - through an exact `(level, name)` heading address. The walk is level-independent: it never descends on its own, but a jump to a child starts a child-level walk under identical rules, and the parent walk resumes after the jumper when that level exhausts. `execute()` runs a contained chain with every normal walk rule and returns the chain's final reply; how a section was entered no longer gates what its control flow may do. `fanout(worker, collection)` takes any Lua table: array members arrive as the arm's `item` with their types intact, hash members arrive as pair tables, and `list_from_section(heading)` feeds a list section's pre-parsed items straight in. The retired two-string fanout form fails loudly at the call site, naming its replacement.

Each change removes a special case: one marker instead of a skip flag plus a comment syntax, one resolver instead of two, one chain function instead of a walk plus a subroutine policy, one collection form instead of two parameter shapes.

## Key design choices

1. **One marker, two positional roles.** A `---` rule as a section's first content (only whitespace before it) takes the section off the walk: skipped in fall-through order at every walked level, still in the section tree, still counted by `sys.section_count`, and still runnable when addressed. Anywhere else, the rule is a comment boundary: nothing below it compiles, reaches the model, or parses as list items, until the next heading. The two roles compose - a section may carry the off-walk marker at the top and a later rule starting a comment region. One token serves both needs because position already disambiguates intent; the rejected alternatives were a comment-only rule (no home for shared sections) and a skip flag (no reader-only region for prose and inert example code). Reversing the positional rule later means re-authoring every shared worker and list section, which is exactly the population the marker exists to serve.

2. **The walk never descends; only addressing does.** A section's children never run in fall-through order. A jump to a direct child starts a child-level walk over the jumper's children, beginning at the target and falling through to following siblings under the same rules as the top-level walk, off-walk skips included; when that level exhausts, the parent walk resumes after the jumper. The rule recurses to the deepest heading levels. The rejected alternative - run the one child, then resume - gives the child level different fall-through rules than the top level, which is the inconsistency this design exists to remove. Because heading levels are bounded 2 through 6, descent is structurally limited to a few frames and needs no separate recursion guard.

3. **`execute()` is a contained chain, not a subroutine with a policy.** An `execute()` call runs a walk starting at its target with every normal rule - fall-through, off-walk skips, jumps, child chains - while the outer walk never moves. When the chain ends, its final reply is the call's return value; a `return` ends only the chain it fires in, and the top-level chain's return ends the run. Nesting is capped at 8. The retired alternative rejected jumps inside execute-called sections, so the same section behaved differently depending on how it was entered; entry mode no longer gates capability. The authoring consequence: a multi-section subroutine is expressed as a child walk (the children need no marker, since no walk descends on its own) or placed after the run-ending section, because a marked continuation is excluded from every chain's fall-through.

4. **One visible set governs every section address.** `jump`, `execute`, `fanout`'s worker, and `list_from_section` all resolve against the same set: the caller's siblings at its nesting level, minus the caller itself, plus the caller's direct children. The parent, aunts and uncles, nieces and nephews, grandchildren, and the caller itself resolve as not-found. The narrower candidates each fail a real use: children-only scoping means two sibling sections could never share one worker, and top-level-only scoping means nothing could address a child. The sharing pattern falls out of the set: a subroutine shared by multiple clients is made their sibling and marked off-walk, which is what the marker is for.

5. **Addresses are exact `(level, name)` pairs.** A heading string like `### Worker` parses into a level and a name, and both must match exactly; the marker count is what disambiguates a sibling from a same-named child in the mixed visible set. Zero matches and more than one match are both errors - never a silent first hit. The parser already forbids duplicate sibling names, so the resolver's loud ambiguity error is a guarantee about the resolver, not an event a real prompt can produce. A not-found error lists only the visible sections, so the error channel cannot leak the rest of the document's structure. The address grammar is the markdown the author already wrote; no parallel naming scheme exists.

6. **The reply thread follows control flow.** The model reply rolls forward through fall-through, across a jump into its target, into and out of child-level walks, and a contained chain's final reply becomes the calling section's `execute()` return value. A sub-walk detour extends the thread rather than breaking it: the jumper's reply is what the addressed child sees, and the exhausted level's last reply is what the section after the jumper sees. `sys.id` counts sections entered within each chain from 1, so a contained chain reads like a fresh run; `sys.section_count` stays the top-level count, a document fact rather than a walk position.

7. **`fanout`'s second parameter is always a collection.** Any Lua table is a collection; a non-table second parameter is an error whose message points at `list_from_section`. The old two-string form - `fanout(worker, list_heading)` - is removed, because two ways to do one thing invite the question of which is correct, and a bare string is ambiguous against a section name. The engine major stays `promptforge: 1`: the removed form has no fielded callers, and a stale call fails loudly at the call site instead of silently changing behavior.

8. **`list_from_section` is a separate global, not a fanout parameter mode.** It takes a heading string or a Section handle, resolves against the one visible set, and returns the section's pre-parsed items - bullets or numbered lines, both handled by the same list grammar - as a Lua array of strings. It keeps the no-items error, which is what catches naming a prose section by mistake, and it fails loudly inside a fanout arm and is absent on the H1, same as the other control globals. Extraction and mapping are separate jobs; the composition `fanout("### Worker", list_from_section("### List"))` names both roles at the call site.

9. **Members cross as data, value by value.** Each collection member converts to JSON individually at the call boundary and arrives inside the arm as the corresponding Lua value; a function, userdata, or thread member is rejected at the boundary with an error naming its index. Arms are separate VMs on separate tasks, so only data can cross - and the crossing rides the same serialization bridge that seeds `var`, one path instead of two. A string member therefore produces a string `item`, exactly as before the collection generalization.

10. **Hash members arrive as pair tables, and hash order is undefined.** The array part (`1..#t`) iterates in order first; the hash part follows in undefined order, and each hash member arrives as `{key = k, value = v}`. Keys must be strings, numbers, or booleans; any other key type is a loud error. The pair shape loses no information - the rejected value-only alternative made keys unrecoverable inside the arm and broke set-style tables - and undefined order is an explicit non-promise, keeping stable ordering cheap to add later and expensive to take away once relied upon.

11. **`item` keeps its type end to end.** In prose, `{{ item }}` renders by type: strings verbatim, numbers and booleans in their natural string form, tables as compact JSON, null as `null`. Erroring on non-string substitution was rejected: it would make rich items unusable in prose for no gain in safety. On the way back, each arm result's `.item` carries the member value as a Lua value - pair tables for hash members - so the caller correlates results with rich items by value instead of by a flattened string.

12. **Edge cases fail where the confusion actually lives.** An empty collection returns an empty result table rather than an error, because mapping over zero items is legitimate; the wrong-section mistake is caught by `list_from_section`'s no-items error, the one place that can distinguish it. The item cap bounds the collection's member count before any arm is scheduled. `sys.taskid` stays the 1-based collection position. An arm that exhausts its tool-loop budget soft-degrades to a stub result that still renders its item; a fatal arm error aborts its siblings. None of these failure modes is new - the generalized surface rides the existing contract.

## The marker rides CommonMark instead of fighting it

The marker is recognized only as a genuine thematic break, so the authoring rule has one consequence: after a prose line, the rule needs a blank line before it, because a text line immediately followed by `---` is a setext heading underline, not a marker. After a heading or a code fence the rule stands alone, and a `---` inside a fence is code, never a marker. Headings below a rule still split sections - the comment region ends at the next heading, not at the end of the file. On the H1 only the comment role applies, because the H1 is never walked: the description text comes from above the rule, and a shared-library fence below it is inert. The marker is blanked rather than cut out, so Lua error line numbers still point at the authored file. One parse rule, no heuristics, no special cases - the design borrows CommonMark's semantics instead of layering a second grammar on top of them.

## One chain function is the whole engine

Structurally, there is one recursive walk over a sibling slice, and every entry mode drives it: the top-level walk runs the prompt's sections, a jump to a child recurses into the jumper's children, and an `execute()` call runs a contained chain over the target's slice from the target's index. The walk parameterizes only the divergences - the `sys.id` counter, the cross-section reply carried in, the execute depth - so the section lifecycle, the prose tool loop, and every observation exist exactly once. A jump within a level is a flat position move; a jump into a child level holds the parent position while the child level runs. A chain ends in exactly two ways, and what an ending means depends only on whose chain it is:

| Chain end | Top-level chain | `execute()` chain |
|---|---|---|
| Level exhausts | The run's result is the last reply, else a generic completion | The call returns the chain's final reply (empty when none) |
| `return` fires | The value is the run's result | The value is the call's return value |

A `return` propagates out of every sub-walk to the root of the chain it fired in, so a child walk cannot accidentally end the run, and the top-level chain's return always can. Arrival by addressing runs the target even when it is marked off-walk; only fall-through arrival applies the skip. That single rule is what makes an off-walk section runnable at all, and it holds identically at every level.

## Four verbs share one address grammar

The control surface is four globals with one targeting convention. A target is a heading string (`"### Name"`) or a Section handle from `tasks`; both resolve through the same path against the same visible set.

```lua
jump(target)                          -- transfer control; never returns
execute(target, input?) -> string     -- contained chain's final reply; input overrides args
fanout(worker, collection) -> array   -- worker is a heading string; results in collection order
list_from_section(target) -> array    -- the section's pre-parsed items as strings
```

Inside a fanout arm, `execute`, `fanout`, and `list_from_section` are loud stubs that name the restriction, and none of the control globals exist on the H1. An arm's environment adds two names: `item`, the member value, and `sys.taskid`, the member's 1-based collection position. Each arm result carries `.text`, `.ok`, `.item`, and `.exhausted`, and coerces to `.text` under `tostring`, so `table.concat` over results keeps working. These names are prompt-facing vocabulary: every prompt that fans out or transfers control speaks them, so they were chosen to say what they do, and renaming any of them later means editing every prompt.

## Collections carry data, never behavior

The collection contract is one iteration rule and one arrival rule. Iteration: the array part in order, then the hash part in undefined order; results come back in collection order, not finish order. Arrival:

| Collection member | Arrives in the arm as |
|---|---|
| Array-part value | Itself, type preserved |
| Hash-part pair | A pair table, `item.key` / `item.value` |
| Function, userdata, or thread | Call-boundary error naming the member's index |
| Non-scalar hash key | Call-boundary error naming the key's type |

The boundary rejects behavior rather than attempting to serialize it, because a member crosses into another VM and only data can make that crossing. An empty collection returns an empty table, and an oversized one errors before any arm is scheduled. The cap and the concurrency window remain host configuration, so the prompt expresses intent and the host controls cost.

## What reversing the headline choices would cost

The vocabulary - `jump`, `execute`, `fanout`, `list_from_section`, `item`, `sys.taskid`, and the `---` marker itself - is spoken by every prompt that uses it, so renaming is the ordinary, accepted cost of a name. The deep reversal is the contained chain: subroutines will be authored as child walks and post-run blocks against the rule that entry mode never gates capability, and re-gating later breaks fielded prompts. The pair-table shape is likewise wired into result-correlation code; adding stable hash ordering later is additive, but changing the pair shape is not. The visible set is the cheapest headline to revisit - widening it changes only what resolves - and the most dangerous, because the not-found error's no-leak property depends on the set being exactly what the caller can already see.

## Deferred until use proves them

Two additions stay out until evidence asks, and both are additive against the surfaces they extend: stable hash-member ordering, which would sort keys at the call boundary if correlation code ever wants it, and streaming or lazily materialized collections, which the member cap already makes unnecessary as a bounding mechanism.

*2026-08-18 21:00 - Kimi K3*
