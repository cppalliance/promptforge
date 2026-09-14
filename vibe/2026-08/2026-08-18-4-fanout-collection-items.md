---
name: fanout collection items
overview: "Eight steps, one commit each: (1) the --- marker - off-walk sections at the top, comment regions elsewhere; (2) refactor the arm's item from String to a JSON value end-to-end with zero behavior change; (3) add the list_from_section(heading) global with the shared visible set; (4) one resolver for jump/execute - resolve_sibling replaces resolve_h2_section, duplicates error loudly; (5) level-independent walks - jump/execute address children, sub-walks, off-walk at every level; (6) execute becomes a contained chain - JumpPolicy retired, the execute adapter collapses into the chain function; (7) make fanout's second parameter always a collection - arrays in order, hash tables in undefined order with pair-table members; (8) generate the design document."
todos:
  - id: step-1-comment-rule
    content: "Step 1: the --- marker - off-walk sections and comment regions (parser, walk condition, tests, docs, Verify)"
    status: completed
  - id: step-2-item-json
    content: "Step 2: arm item becomes a JSON value (refactor, no behavior change)"
    status: completed
  - id: step-3-list-from-section
    content: "Step 3: list_from_section(heading) global - resolution, scoping, tests, docs"
    status: completed
  - id: step-4-resolver-unification
    content: "Step 4: one resolver for jump/execute - resolve_sibling replaces resolve_h2_section, duplicates error loudly"
    status: completed
  - id: step-5-level-walks
    content: "Step 5: level-independent walks - jump/execute address children, sub-walks, off-walk at every level, tests, docs"
    status: completed
  - id: step-6-contained-chains
    content: "Step 6: execute becomes a contained chain - JumpPolicy retired, adapter collapses into the chain function, tests, docs, Verify"
    status: completed
  - id: step-7-collection-form
    content: "Step 7: fanout's second parameter is always a collection - conversion, hash pairs, migration, tests, docs, Verify"
    status: completed
  - id: step-8-design-doc
    content: "Step 8: generate design-promptforge-section-marker-and-fanout-collections.md via the design-doc block"
    status: completed
isProject: false
---

# PromptForge: the section marker and fanout over collections

## Target

Two cooperating features. First, the `---` marker: as a section's first content it takes the section off the walk (it runs only when addressed); anywhere else it starts a reader-only comment region. This gives shared workers and list sections a clean home as top-level sections. Second, `fanout(worker, list)` currently takes two section-heading strings and maps the worker over a list section's pre-parsed bullet items, with each arm's `item` a string; this change splits that in two: a new global `list_from_section(heading)` returns a section's pre-parsed items (bullets or numbered, both handled by the existing parser) as a Lua array of strings, and `fanout`'s second parameter becomes always a collection - an array whose members arrive as `item` Lua values, or a hash table whose members arrive as pair tables. The two-string form becomes `fanout("### Worker", list_from_section("### List"))`.

## Decisions

0. **The `---` marker has two roles, disambiguated by position** (user design, recovered from memory across two corrections - no written record exists in the code, the design repo, or the last 14 days of transcripts). As the first content of a section (only whitespace before it), it marks the section **off-walk**: the top-level walk never visits it, and it runs only when addressed by `execute`/`jump`/`fanout`/`list_from_section`; content below the marker is fully executable. This is how shared workers and list sections live as top-level sections without running in the walk - the user's example: `## A` running `execute("## B")` and `execute("## C")` with `## B` and `## C` marked off-walk and `## D` unmarked, so the walk visits A then D. Anywhere else, the rule is a **comment boundary**: everything below it until the next heading is reader-only - no Lua compiles or runs, no prose reaches the model, no items parse from it. The two roles compose: a section may carry the off-walk marker at the top and a later rule starting a comment region. On the H1 the off-walk role is meaningless (the H1 is not walked), so a first-content rule there is simply a comment boundary. Headings below a rule still split sections. The off-walk role applies at whatever level a walk runs: an H3 marked off-walk is skipped by an H3-level walk (decision 12) and stays addressable. Rejected: comment-only (fails the off-walk use case) and skip-flag-only (loses reader-only prose and inert example code). Authoring note (user decision - no special casing): the marker is recognized only as a genuine CommonMark thematic break, so after a prose line it needs a blank line before it - a text line immediately followed by `---` is a setext heading underline, not a rule, and the parser does not special-case it. After a heading or a fence it stands alone.

1. **fanout's second parameter is always a collection.** The string form moves out to `list_from_section`; a non-table second parameter errors with a message pointing at it. Rejected: keeping the string form beside the collection form (two ways to do one thing, and a bare string is ambiguous against a section name).
2. **`list_from_section(heading)` is a global beside `execute`/`jump`/`fanout`, with a strict visible set** (user decision): sibling sections at the caller's nesting level (same parent; for the walk and `execute()` subroutines that is the other top-level sections, excluding the caller itself) plus the caller's direct children. The parent, aunts/uncles, nieces/nephews, grandchildren, and the caller itself are not visible - they resolve as not-found, and the error lists only the visible sections so the error channel does not leak the rest of the document's structure. The `(level, name)` heading address disambiguates the mixed set (`## List` matches only a sibling, `### List` only a child). It takes a heading string or Section handle (via `resolve_section_target`), returns the section's pre-parsed items as a Lua array, and keeps the "no pre-parsed items" error - that error catches naming a prose section by mistake. It fails loudly inside fanout arms and is absent on H1, same as the other control globals.
3. **One visible set governs every section-addressing function** (user decision, unified across the conversation): `list_from_section`, `fanout`'s worker, `execute`, and `jump` all resolve against the same set - sibling sections at the caller's nesting level (same parent; for the walk and `execute()` subroutines that is the other top-level sections, excluding the caller itself) plus the caller's direct children. With children-only scoping, two sibling sections could never share one worker; with H2-only scoping, nothing could address a child. A running H3's visible set is its own siblings and children: it cannot jump out to a top-level section and exits by falling through. The sharing pattern (user decision): a subroutine shared by multiple clients is made their sibling and marked off-walk; a multi-section subroutine is a shared sibling that descends into its own child walk. A shared subroutine entered via `execute` can jump into its child walk like any other section (decision 14) - entry mode no longer gates capability. Consequence: a worker shared as a top-level section stays out of the walk via the off-walk marker (decision 0) - which is what the marker is for.
4. **Members cross as JSON, value by value.** Arms are separate VMs on separate tokio tasks, so members serialize; the bridge is the same JSON path `var` already uses. The call iterates the Lua table and converts each member individually (whole-table serde cannot represent mixed tables). A function or userdata member errors at the call boundary naming the index. Rejected: arbitrary Lua-value passing (impossible across VMs without exactly such a bridge).
5. **`item` arrives as the member's Lua value.** A string member produces a string `item` - identical to today. Tables and scalars convert through the same serde bridge used to seed `var`.
6. **Hash tables are accepted; their order is undefined** (user decision). Iteration is the array part (1..#t) in order first, then the hash part in undefined order. A hash member arrives as a pair table: `item.key` and `item.value` (user decision) - no information lost, and set-style tables stay meaningful. Keys must be JSON scalars (string, number, boolean); a table or function key errors loudly. Rejected: value-only members (keys unrecoverable inside the arm).
7. **`{{ item }}` prose substitution renders by type:** strings verbatim, numbers and booleans via their natural string form, tables as compact JSON, and null as `null` (extended at step 2 review: `render_item` covers every JSON type, so null is named too). Rejected: erroring on non-string substitution - it would make rich items unusable in prose for no gain in safety.
8. **`.item` on each arm result carries the member value back** (as a Lua value via the same bridge; pair tables for hash members). String members are unaffected in practice. This lets the parent correlate results with rich items (e.g. the dissected section record) instead of a flattened string.
9. **An empty collection returns an empty result table.** Rejected: erroring - mapping over zero items is legitimate; the wrong-section mistake is caught by `list_from_section`'s no-items error instead.
10. **The existing caps apply unchanged:** `max_fanout_items` bounds the collection length, `sys.taskid` stays the 1-based iteration position, the exhausted stub renders the item per decision 7.
11. **Refactor first, feature second.** Step 2 changes the arm's item representation from `String` to JSON with string members only - behavior-identical, existing suite stays green. Steps 3-7 add the new behavior on top, so their diffs are purely additive. The engine work splits three ways (steps 4-6) so each commit is one testable behavior: resolver unification, then level walks, then contained chains - ordered so no commit leaves resolution and machinery in a broken intermediate state.
12. **Control transferred into a child level starts a walk at that level** (user decision: identical rules, different level - the engine's section semantics are level-independent; the user frames the purpose as "a section can opt-in to expressing a subroutine as a child walk," and the detour does not break the reply thread, it extends it). A walk never descends on its own: the transfer from level N to level N+1 is always explicit (`jump`/`execute`), and only after the transfer does the sibling walk at the deeper level proceed normally. The top-level walk never falls through from an H2 into its H3 children. A jump to an H3 child runs an H3-level walk within the jumper's children, starting at the target and falling through to following siblings exactly as the top-level walk does at H2; when the level exhausts, the parent walk resumes after the jumper. The rule is recursive - an H4 level behaves the same way. The `reply` thread follows control flow through the detour: the jumper's reply rolls into the sub-walk's first section (as it does into a jump target today), each section of the sub-walk rolls forward as usual, and the sub-walk's last reply resumes the parent chain - so in the plan's example, C's incoming `reply` is Y's output. Rejected: run-the-child-then-resume (a single-section detour) - it gives the H3 level different fall-through rules than H2, which is the inconsistency this decision exists to remove. `execute` stays a subroutine at every level; what its target's control flow may do is decision 14.
13. **One resolver replaces two.** `resolve_sibling` (exact `(level, name)`, ambiguity errors) replaces `resolve_h2_section` (name-only, silent first match) for `jump`/`execute`. Side effect: two sections sharing a name within the visible set now error loudly instead of silently resolving to the first. As built (step 4): a new `resolve_top_level_index` helper serves the walk's index move, and the malformed-heading error changed from "must use ## markers" to a not-found listing of the visible sections - inherent to one resolver, and step 5 makes the old message wrong anyway (children become addressable).
14. **`execute()` starts a contained chain; jump works everywhere** (user decisions, with canonical examples). The user's specs: `## A` calls `execute("## Sub")`; `## Sub` calls `jump("### S1")`; S1 falls through to S2; when S2 finishes, the reply is returned to A and execution continues. And: A calls `execute("## S1")` (S1 off-walk); S1 falls through to S2; S2's reply returns to A - with the main walk ending at B, so the subroutine sections run only inside the chain. The rule: an `execute()` call runs a contained chain starting at its target - a walk with every normal rule (fall-through, off-walk skips, jumps, child chains) - and when the chain ends (the level exhausts or a return fires), its final reply is the call's return value. In the user's framing: like calling a different prompt whose pieces happen to be in the same file, and recursive - a chain can execute another chain (depth-capped). A return ends the chain it fires in; the top-level chain's return ends the run. The outer walk never moves. `JumpPolicy::Reject` is retired - a section's capabilities no longer depend on how it was entered. Control-flow coherence (infinite loops, wrong targets) is the prompt author's responsibility, as in any programming language (user decision). The contained chain skips off-walk sections in fall-through like any other walk (user decision: consistency - a marked section is skipped everywhere; only addressing runs it). Consequence for subroutine authorship: a multi-section subroutine is expressed as a child walk (the children need no marker, since no default walk ever descends to them); a sibling fall-through subroutine cannot be hidden mid-document, because marking the continuation excludes it from the chain - the flat alternative is to place the subroutine block after the run-ending section. Rejected: the single-section contained flow (fails the user's fall-through example), the reject policy (the entry-dependence asymmetry the user called out), and execute chains traversing off-walk blocks (breaks the consistency rule).

## Execution protocol

- Per `tools-public/rulebooks/vibe-rulebook.md`: one testable commit per step carrying code, test, and docs; coder subagent dispatched with the plan path and step number; review-and-fix applies `<code-review>` from that file plus the plan-local `<debt-review>` block below, overwriting `cabinet/_scratch/vibe-fanout-collections/vibe-review.md`; amend on a dirtied tree.
- Per `tools-public/rulebooks/rust-rulebook.md`: rustdoc with `# Errors` on new fallible items, no `unwrap` outside tests, `cargo fmt --all --check` and `cargo clippy -p promptforge-core --all-targets --all-features -- -D warnings` green before each commit. NOTE: master carries pre-existing fmt drift (recorded in the prior plan's Found debt) - `cargo fmt --all` must not sweep those files into a step commit; revert out-of-scope formatting.
- Decision currency: where a step's implementation contradicts, extends, or resolves a decision recorded here, the step revises this plan in the same commit, naming what forced the change.
- Verify (workspace `cargo test`) runs on steps 1, 3, 6, 7, and 8 (every third step, end of each component, and the final step). The core-tests fanout suite covers the collection feature; no live smoke is needed since no shipped prompt uses the form yet.
- Tests cover absence as well as presence: non-array table errors, function/userdata member errors naming the index, wrong-type second parameter errors.

<debt-review>
For every file the step's diff touches, review the whole file, not just the diff:
1. Is every function in the file needed? Name any candidate for removal and why it is dead.
2. Can any functions be combined without obscuring them?
3. Is any function obsolete given this step's change - superseded, duplicated, or unreachable?
4. How does the file interact with the other files this step touched or the next step will touch? Name any coupling that can be simplified.
5. What simplification is available that the diff did not take?
6. Do the tests cover the absence of behavior - removed names erroring, blocked actions failing, invalid inputs rejected - and not only the presence of new behavior?
Fold each finding that stays inside the step's file set into the same commit. Record anything larger in this plan under Found debt with the file, the finding, and the estimated size; do not fix it in passing.
</debt-review>

## Found debt

- **Setext underlines may create phantom sections** (pre-existing parser question, surfaced while designing decision 0): a prose line immediately followed by `---` is a CommonMark setext H2 underline, which the heading scanner may read as a new section. The user ruled no special casing - the blank line before the comment marker is required - so step 1 pins the current behavior with a test and does not fix it. Size: small.
- **`run_one_arm` duplicates most of `run_one_section`'s block lifecycle** (prologue, prose, tool loop, epilog), with arm-specific deltas (item injection, taskid, stubbed control globals, the finalizer, the cancel scope, the exhausted stub). The level-independent walk does not fix this; it is the next collapse candidate after this plan. Size: medium.
- **`resolve_sibling`'s error text says "fanout heading ..." even when reached via `list_from_section`** (found at step 3 review): the caller name and the "available siblings" wording are wrong for the new caller, and the visible set includes children. Step 7 opens `fanout/mod.rs` - fix there: a caller-label parameter or context-neutral wording. Size: trivial.
- **`promptforge.md`'s quickref still says execute runs a section "as a subroutine"** (found at step 6 review): now misleading under the contained-chain contract. Step 7 opens `promptforge.md` - fix the execute row there. Size: trivial.
- **`guide/promptforge-user-guide.md` still says jump inside execute "is rejected" and execute "runs a section as a subroutine"** (found at step 6 review): both false now. This file is GENERATED by make-user-guide. Correction (found at step 7 review): its input `crates/promptforge-core/user-guide-promptforge-core.md` is itself stale (still documents the removed two-string fanout form), so regeneration alone propagates the staleness - the per-crate chapter needs the edit first, then regenerate. Size: small.
- **`crates/promptforge-core-tests/prompts/invalid/list-h3-with-lua.md` is referenced by no test** (found at step 7 review): the step-7 migration preserved an unexercised fixture. Pre-existing deadness, not introduced here. Plan-owner call: wire it in or remove it. Size: trivial.
- Everything else: populated by the `<debt-review>` block as steps run.

## Impact map

Every source file in the crate was swept against the rules (four parallel batches). Affected files, by owning step:

- Step 1: `parser.rs` (`Section.off_walk` + accessor), `parser/build.rs` (Rule seam in `build_sections`), `parser/fence.rs` (`split_h1` comment role), `execute/engine.rs` (walk skip), `parser/tests.rs`.
- Step 2: `fanout/arm.rs` (`ArmPayload.item`), `fanout/mod.rs` (`run_fanout_arms` items slice), `lua/handles.rs` (`LuaFanoutResult.item`; drop `Eq`), `lua/vm.rs` (item injection via `to_value`), `subst.rs` (item becomes `Option<&Value>`).
- Step 3: `lua/vm.rs` (`list_from_section` global + callback), `execute/engine.rs` (the shared visible-set helper).
- Step 4: `execute/engine.rs` (retire `resolve_h2_section`/`resolve_h2_index`), `execute/tests/exec_flow.rs` (duplicate-error test).
- Step 5: `execute/engine.rs` (caller-local visible set; sub-walk on jump-to-child), `lua/vm.rs` (callback plumbing).
- Step 6: `execute/engine.rs` (execute adapter collapses into the chain function; `JumpPolicy` removed), `execute.rs` (module rustdoc), `execute/tests/exec_flow.rs` (canonical examples).
- Step 7: `lua/vm.rs` (fanout closure signature), `execute/engine.rs` (`make_fanout_callback`), `fanout/mod.rs` (module docs), `subst.rs` (rendering rule), `execute/tests/exec_flow.rs` + `live_infer.rs` + `model_and_reply.rs` (two-string migrations), `fanout/tests.rs`.

Verified unaffected by reading, not assumption: `store/*`, `model/*`, `tools/*`, `client/*`, `dialects/*`, `normalize.rs`, `untrusted.rs`, `cancel.rs`, `debug.rs`, `observe.rs`, `error.rs`, `resolve.rs`, `lib.rs`, `lua/host.rs`, `lua/program.rs`, `lua/live.rs`, `lua/scope.rs`, `lua/sys.rs`, `lua/hardening.rs`, `lua/tools_bridge.rs`, `lua_models/*`, `execute/{h1,tool_loop,tools,scope,support,gateway,error,config}.rs`, `fanout/proxies.rs`, and the remaining `execute/tests/` files.

Assumption corrections the sweep surfaced (all folded into the steps): `resolve_sibling` already lives in `fanout/mod.rs` - adopt it, don't invent it; `run_execute_section` resolves against the cloned top-level list today, so the visible set must be threaded caller-local; `LuaFanoutResult` derives `Eq`, which `serde_json::Value` lacks; `subst.rs` carries `{{ item }}` and no step named it; `Prompt::entry()` may need off-walk awareness.

## Steps

### Step 1: the `---` marker - off-walk sections and comment regions

- Code (`crates/promptforge-core/src/parser/` plus one walk condition in `execute/engine.rs`): per content region, if the first pulldown `Rule` event precedes any executable content (only whitespace before it), the section is marked off-walk (a flag on `Section`) and the content below parses normally; otherwise the first Rule ends executable content - everything below is a reader-only comment (no Lua compiles, no prose reaches the model, no items parse from it). The two compose: an off-walk marker at the top, then a later Rule starts a comment region. The H1 region takes only the comment role (a `lua shared` fence below the rule is inert; `description_text` comes from above the rule). Headings below a rule still split sections. List-section items parse from the section's executable content (below an off-walk marker, above a comment boundary). The walk in `run_sections` skips off-walk sections entirely (no `SECTION_STARTED`, no execution); they stay in the section tree and addressable. `sys.section_count` still counts them - they are still sections. No special casing of setext underlines: the marker is only a genuine `Rule` event (user decision - the blank line after prose is required). The Rule-detection seam is `build_sections` (and `split_h1` for the H1), truncating each heading's content before `split_section_blocks`/item parsing - not the heading scanner, which already stops content at the next heading. Check `Prompt::entry()` callers: if the walk starts from `entry()`, it must start at the first non-off-walk section. (Resolved as built: the walk starts from `prompt.sections` index 0, not `entry()`, and all `entry()` callers are tests - no change needed.)
- Test: an off-walk section is never visited by the walk but runs via `execute`/`jump`/as a fanout worker; content below the off-walk marker executes; the user's exact example shape (A executes B and C, both off-walk, D unmarked - walk visits A then D); the composition shape (off-walk marker, Lua fence, prose, blank line, comment marker, comments); a comment region excludes prose/Lua/items; an off-walk list section's items parse from below the marker; the H1 comment role; a `---` inside a fenced code block is not a rule (pulldown handles this); the setext case (prose line immediately followed by `---`, no blank line) is pinned as whatever it currently is - see Found debt.
- Docs: `guide/src/prompt-files.md` (both roles with the blank-line authoring note), `promptforge.md` quickref (Prompt Structure section).
- Verify: workspace `cargo test` (end of the parser component).

### Step 2: arm item becomes a JSON value (refactor, no behavior change)

- Code: in `crates/promptforge-core/src/fanout/arm.rs`, `ArmPayload.item_text: String` becomes `item: serde_json::Value`; `run_one_arm` installs it via a JSON-to-Lua conversion (the `LuaSerdeExt` bridge used for `var`) instead of `set_global_string`, and computes the substitution rendering per decision 7 for `subst::substitute` and the exhausted stub. In `crates/promptforge-core/src/lua/handles.rs`, `LuaFanoutResult.item` becomes a JSON value whose userdata getter returns the corresponding Lua value via `to_value` - note the derive: `LuaFanoutResult` currently derives `Eq`, which `serde_json::Value` does not implement, so the `Eq` derive comes off. `run_fanout_arms` (`fanout/mod.rs:147`) takes `items: &[serde_json::Value]`; the engine's list-section path (`execute/engine.rs:754-816`) converts the pre-parsed `Vec<String>` to JSON strings at the boundary. The fanout callback type in `install_control_globals` (`lua/vm.rs:471`) stays `Fn(String, String)` here (as built: the Lua-facing two-string form is unchanged until step 7; only the item plumbing changes - a new `set_global_json` beside `set_global_string`). `subst.rs` (found by the sweep - no step named it): the `item` parameter becomes `Option<&serde_json::Value>` with the decision-7 rendering (a shared `render_item` helper used by `substitute` and the exhausted stub).
- Test: the entire existing fanout suite (`fanout/tests.rs`, `promptforge-core-tests` fanout suite) stays green unchanged - that is the proof of no behavior change.
- Docs: none (internal only).

### Step 3: `list_from_section(heading)` global

- Code: new global installed beside `execute`/`jump`/`fanout` in `install_control_globals` (`crates/promptforge-core/src/lua/vm.rs`), backed by a callback carrying the visible set (same plumbing as the fanout callback in `execute/engine.rs`): the top-level sections minus the caller (by index), plus the caller's `children`. Takes a heading string or Section handle (via `resolve_section_target`), resolves with `fanout::resolve_sibling` over the visible set, returns the section's pre-parsed `items` as a Lua array of strings. Keeps the "no pre-parsed items" error; the not-found error lists only the visible sections. In fanout arms it fails loudly ("list_from_section() is not available inside a fanout arm", matching the execute/fanout stubs in `fanout/arm.rs`); it is not installed on the H1 VM.
- Test (`fanout/tests.rs` or the nearest control-globals tests): bullet and numbered lists both return their items; a sibling section is visible; a direct child is visible; a sibling's child (niece/nephew) is not visible; a grandchild is not visible; the caller itself is not visible; not-found and ambiguous errors list only the visible sections; a prose section errors with the no-items message; calling it inside an arm errors; absence on H1; the flagship composition - a sibling list section marked off-walk returns its items through `list_from_section` and never walks.
- Docs: `guide/src/lua.md` API table, `guide/src/prompt-files.md`, `promptforge.md` quickref.

### Step 4: one resolver for jump/execute (refactor, loud duplicates)

- Code (`execute/engine.rs`): `resolve_sibling` replaces `resolve_h2_section` and `resolve_h2_index` for `jump` and `execute`, resolving over the top-level slice - H2 behavior unchanged (decision 13). Duplicate names in the set now error loudly instead of silently resolving to the first. The visible-set helper from step 3 exists, but jump/execute stay top-level-only here; widening is step 5.
- Test: the existing jump/execute suite stays green; two same-named sections error loudly.
- Docs: none (internal; the guide rows update in steps 5-6).

### Step 5: level-independent walks (jump/execute address children)

- Code (`execute/engine.rs`, `lua/vm.rs`): the shared visible-set helper (siblings plus children) threads into the jump/execute callbacks caller-local - today `run_execute_section` and the callbacks resolve against the cloned top-level list, so the engine must thread the running section's own siblings-plus-children into the callbacks (one helper in `engine.rs`, used by `jump`, `execute`, `fanout`, and `list_from_section`). `jump`/`execute` accept any heading in the visible set; `### Child` resolves to a direct child. The walk's jump handling (`SectionFlow::Jumped`) gains the child case per decision 12: control enters a child-level walk within the jumper's children, starting at the target and falling through to following siblings with the same rules as the top-level walk; when the level exhausts, the parent walk resumes after the jumper; recursive for H4 and deeper. A top-level target keeps today's index move. The off-walk flag applies at every walked level. `sys.section_count` stays the top-level count (a document fact); `sys.id` keeps counting sections entered run-wide. `execute` to a child still runs it as a single-section subroutine here; the contract change is step 6.

  Implementation shape (user question, answered here): no level parameter N exists anywhere. The walk becomes one recursive function over a sibling slice (`&[Section]`) - the top-level walk runs `prompt.sections`, a sub-walk runs the jumper's `children`. A section's level lives on `Section.level` and a heading's level in its marker count, so resolution and walking never consult a number that could disagree with the tree. A running section's visible set is its own slice minus itself, plus its children - one helper in `engine.rs` shared by `jump`, `execute`, `fanout`, and `list_from_section`. This collapses four special cases into the general rule: `resolve_h2_section` and `resolve_h2_index` retire (step 4), the fanout callback's children-only resolution folds in, and the execute callback's top-level-list resolution folds in. The off-walk skip is one condition in the one walk, so it applies at every level for free. What does not collapse: execute's return boundary (a contained chain returns its final reply to the caller, while jump transfers) is a real semantic difference, and fanout arms stay arms.

  Control state (user question, answered here): the executor's position is a program counter per walk level - today one `index` over the top-level slice (`engine.rs:166`), and under this plan one index per level. A jump within a level stays a flat index move with no stack growth. A jump into a child level is async recursion (`Box::pin`): the sub-walk runs over the jumper's children slice with its own index while the parent frame holds its PC, and the parent resumes after the jumper when the level exhausts. No new recursion cap is needed - the parser bounds heading levels 2 through 6, so descent is structurally limited to four frames. `execute` nesting continues to ride the Rust stack through `bridge_blocking` under `MAX_EXECUTE_DEPTH`, unchanged. Rejected: an explicit `Vec<(slice, index)>` frame stack - it moves the same state off the Rust stack for no gain at this depth.
- Test: jump to an H3 child starts the child-level walk at the target and falls through to its following siblings; exhaustion resumes the parent walk after the jumper; the reply thread follows the detour (the jumper's reply reaches the first child, the last child's reply reaches the section after the jumper); the rule recurses to H4; an off-walk H3 is skipped by the child-level walk but runs when addressed; execute to an H3 child runs it single-section (the contract change is step 6); the H2 walk never descends - a section's children do not run unless addressed; a running child can address its own siblings and children; a running child cannot address a top-level section (not in its visible set); a niece/nephew target errors; H2 sibling targets behave exactly as today.
- Docs: `guide/src/lua.md` (jump/execute accept any heading in the visible set; the level-independent walk rule), `guide/src/prompt-files.md`, `promptforge.md` quickref.

### Step 6: execute becomes a contained chain (JumpPolicy retired)

- Code (`execute/engine.rs`): the execute adapter and the top-level walk collapse into one recursive chain function over a sibling slice from a start index (decision 14) - an `execute()` call runs a contained chain with every normal rule and returns the chain's final reply when the chain ends (the level exhausts or a return fires). A return ends the chain it fires in; the top-level chain's return ends the run. The `JumpPolicy` enum, its `WalkContext` parameter, and the reject arm at `engine.rs:426-431` are deleted; the "unreachable" `SectionFlow::Jumped` arm in `run_execute_section` goes with them. `execute.rs`'s module rustdoc still describes the H2-only walk - update it.

  The simplification this step delivers (user requirement - the plan must simplify, because the special cases are eliminated): the `JumpPolicy` enum, its `WalkContext` parameter, and the reject arm at `engine.rs:426-431` are deleted; `run_execute_section`'s separate subroutine policy collapses into the general chain (the "unreachable" `SectionFlow::Jumped` arm goes with it); `resolve_h2_section` and `resolve_h2_index` were deleted in step 4 (decision 13); the execute/jump capability asymmetry disappears. What survives: `run_one_section` as the universal section-lifecycle engine, now shared by every path with no policy divergence.
- Test: the user's canonical examples (decision 14: A executes Sub, Sub jumps to S1, S1 falls through to S2, S2's reply returns to A, A continues; and A executes off-walk S1, S1 falls through to S2, S2's reply returns to A, the main walk ending at B never runs S1/S2); a jump inside an execute chain to a sibling moves within the contained chain; the outer walk never moves during a contained chain; a contained chain skips off-walk sections in fall-through like any walk (consistency); `jump_inside_execute_is_rejected` inverts into the containment proof; a return inside a chain ends the chain, not the run.
- Docs: `execute.rs` module rustdoc; `guide/src/lua.md` (the execute contract), `guide/src/prompt-files.md`.
- Verify: workspace `cargo test` (end of the engine component).

### Step 7: fanout's second parameter is always a collection

- Code: the fanout closure in `lua/vm.rs` takes `(worker, Value)`; a non-table second parameter errors "fanout's second parameter is a collection; for a list section use list_from_section(heading)". A table iterates array part (1..#t, in order) then hash part (undefined order); each member converts to `Json` individually - array members as themselves, hash members as `{"key": k, "value": v}`; a function/userdata member errors naming its index; a non-scalar hash key errors. `make_fanout_callback` drops the list-section resolution (that moves to `list_from_section`) and resolves the worker over the decision-2 visible set (siblings plus children, the same set step 3 builds) instead of children only; the worker is-a-list-section check stays. Item-cap check applies to the collection length. Migrate the core-tests fanout prompts and `fanout/tests.rs` from the two-string form to `fanout(worker, list_from_section(...))`.
- Test (`fanout/tests.rs` and the core-tests fanout suite): array members arrive as themselves; hash members arrive as pair tables; array order preserved; empty collection returns an empty table; `.item` round-trips members; `{{ item }}` renders per decision 7; a worker resolves as a sibling, one worker is shared by two sibling callers, a child worker still resolves, and a niece/nephew worker is not visible; absence cases: string second parameter errors pointing at `list_from_section`, number/boolean second parameter errors, function member errors naming the index, table-keyed member errors, oversized collection errors.
- Docs: `guide/src/lua.md` (fanout signature and the collection contract), `guide/src/prompt-files.md`, `promptforge.md` quickref.
- Verify: workspace `cargo test`.

### Step 8: design document

After implementation is complete, generate the design document: spawn one subagent whose entire prompt is - read this plan at c:\Users\Vinnie\.cursor\plans\fanout_collection_items_44b6589f.plan.md, grep for `<design-doc>`, and follow the block inside it. Move the generated `design-promptforge-section-marker-and-fanout-collections.md` into `crates/promptforge-core/` beside `design-core.md`.

<design-doc>
OUTPUT A DESIGN DOCUMENT, NOT CODE. Write one markdown file, design-promptforge-section-marker-and-fanout-collections.md,
that explains the design of what this plan describes. You run as the final step
of the plan, after the implementation is complete, so describe the design as
built, reconciling against the finished work any decision the implementation
changed from what this plan first recorded.

NO IMPLEMENTATION CODE - no function bodies, no private machinery, no
step-by-step algorithm walkthroughs. You MAY include any normative artifact the
design needs to remove ambiguity: public signatures, schemas, state or
transition tables, wire formats, configuration syntax, sequence diagrams, and
pseudocode. Each such artifact must express a design contract, not an
implementation technique; include one only where prose cannot say the same
thing as precisely, and show the artifact alone, not the surrounding machinery.

FOR EVERY DESIGN ELEMENT, STATE THREE THINGS: what is observed (by the user or
by an external consumer), how it is structured, and WHY - the motivation, the
rationale, the principle. For a costly-to-reverse element, "why" must include
what reversing it later would cost.

DESIGN-ELEMENT TEST - include something only if changing it would change ANY of:
  (a) ANYTHING THE USER SEES, READS, WRITES, TYPES, OR NAMES. For a library the
      user is the caller, so this is the PUBLIC API - its operations and their
      contracts (ownership, lifetime, thread-safety, error and complexity
      guarantees). It also includes every config file or frontmatter the user
      edits, and - critically - the NAMES of everything the user sees. A name
      is a design decision: `goto` is a good one, `clear_and_transfer_control`
      is a bad one. Naming is design.
  (b) the shape or structure of the system.
  (c) something costly or hard to reverse that the user never sees - the ABI,
      an on-disk or persisted format that outlives a version, a high-reach
      convention that touches everything, or a cross-cutting quality trade-off
      (security, failure modes, data lifecycle, performance).
If it is none of these - merely how you implement the design behind those
surfaces, such as a private helper type, an internal algorithm choice, a
dependency version pin, or a serialization used only between your own
components - it is implementation. Leave it out.

A public interface is design; a private type is implementation - the same
struct is on opposite sides of the line depending on whether the user sees it.
Describe an interface's shape and contract in prose by default; show the actual
artifact - a signature, a schema, a state table - wherever that artifact is
itself the load-bearing decision and prose would blur it. No fixed budget binds
these; each earns its place only by being load-bearing.

COMPRESS BEFORE WRITING - only if the design carries far more ditchable detail
than load-bearing decisions (roughly 10 to 1 or worse). If it is already lean,
skip this. Run the pass in order, cheapest cut first, and stop once the ratio
is healthy:
  1. Drop a default only when changing it would change no observable behavior
     and carry no meaningful risk. A consequential default - a timeout,
     ownership, a security posture, a retry policy, a resource limit, a
     compatibility choice, a failure mode - resolved a real fork and stays.
  2. Move anything decidable later at little or no extra cost to a "decide by
     use" list, or drop it. A cheaply-deferrable element is not a headline one.
  3. Replace an enumeration with the rule that generates it.
  4. Merge consequences into the decision that forces them, and sibling
     elements into their shared pattern.
  5. Name a known pattern instead of re-deriving it.
  6. Rank what remains and keep about 10 to 15 headline elements; demote the
     rest to one line.
  7. Delete anything whose removal would still let a competent builder build
     the right thing.

STRUCTURE - three fixed sections, then whatever the design earns:
  - A title stating what building this produces.
  - An executive summary that stands alone; a reader acts on it without the body.
  - A numbered list of the 10 to 15 key design choices, each a short paragraph.
Then, for a reader who stops early:
  - Write headings that state the point, not the topic ("Labels compute at
    boot, off the critical path", not "Labels").
  - Keep rationale in prose; do not bulletize an argument. Enumerate only
    parallel items (decisions, constraints, options).
  - State the evidence before the value word: never "fast" before the number.
  - Where a choice resolved a real fork, name the alternative and why it lost.
  - Order by importance; put a dependency first only where the reader needs it
    to follow what comes next, so cutting from the bottom never removes the core.
  - Add no YAML frontmatter. Close with one italic line naming the date and the
    model. Name no tool, rulebook, or source document for the document's own
    rules or structure.

CHECK BEFORE FINISHING, and fix any no: no implementation code, and every
normative artifact expresses a contract rather than a technique; every element
states what, how, and why; headings state points; no argument is bulletized;
the compression ratio is healthy; no source document is named. If the plan
carries no key design choices, write no document and return the reason.
</design-doc>

## Out of scope

- Stable ordering for hash-table collections - explicitly undefined by decision 6. If evidence later wants order, sort keys at the call boundary; that is additive.
- Streaming or lazily-materialized collections - members are fully realized at the call boundary; the item cap makes that bounded.