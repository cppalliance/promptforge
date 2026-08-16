---
name: H1-only prompts, remove default_return, fix reply consistency
overview: Five commits that clean up the reply/return story - remove unused default_return, preserve reply across jump(), rename last_reply, allow H1-only prompts, update all docs.
todos:
  - id: step-1
    content: "Commit 1: Remove default_return from frontmatter, executor, tests, and docs"
    status: completed
  - id: step-2
    content: "Commit 2: Preserve reply across jump() - delete the clearing line, update tests and docs"
    status: completed
  - id: step-3
    content: "Commit 3: Rename last_reply to reply in run_sections loop"
    status: completed
  - id: step-4
    content: "Commit 4: H1-only prompts - parser, entry(), LiveH1State.reply, executor, tests"
    status: completed
  - id: step-5
    content: "Commit 5: Doc sweep - update all 11 markdown files, add two worked examples"
    status: completed
isProject: false
---

# Clean Up Reply/Return: Five Commits

## What we are building

Remove the unused `default_return` frontmatter field, make `reply` survive `jump()`, unify the variable naming, and allow prompts with no H2 sections. Five commits, each leaving the tree green.

## Verification

After each commit, run:

1. `cargo check --workspace`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --workspace`

Do not proceed to the next commit until all three pass. On failure, fix in the same commit (amend).

## Review block

<code-review>
Read the diff for the commit named by the step. Apply each check as yes-or-no. For every failure, name the file, line, problem in one sentence, and the fix.

1. Does the change do what the step's intent says, and nothing else?
2. Is every new behavior covered by a test that would fail if that behavior broke?
3. Does the change reuse what already exists instead of rebuilding it?
4. Is every error handled or returned with `?`, never swallowed?
5. Do names, structure, and style match the surrounding code?
6. Is the change free of dead code, unreachable branches, and commented-out lines?
7. Does every `.clone()` have a reason (a second owner is needed), or should it be removed?
8. Are borrow errors fixed by restructuring, not by clone/Arc/unsafe?
9. Does `cargo clippy --all-targets -- -D warnings` pass?
</code-review>

## Step 1: Remove `default_return`

**Intent:** Delete the `default_return` frontmatter field and all code that reads it. The fall-through chain becomes `reply.unwrap_or("done")`.

**Code:**

- [`crates/promptforge-core/src/parser/build.rs`](crates/promptforge-core/src/parser/build.rs): Remove `default_return: Option<String>` and its `#[serde(default)]` from the `Frontmatter` struct.
- [`crates/promptforge-core/src/execute/engine.rs`](crates/promptforge-core/src/execute/engine.rs) line 194-200: Replace `prompt.frontmatter.default_return.clone().or(last_reply).unwrap_or_else(|| "done".to_string())` with `last_reply.unwrap_or_else(|| "done".to_string())`.
- Fix every compile error the removal causes. Follow the compiler; do not guess.

**Tests:**

- Remove `default_return_precedes_the_last_model_reply` in `execute/tests/model_and_reply.rs`.
- Remove `runs_off_end_to_default_return` in `execute/tests/exit_rules.rs`.
- Check `crates/promptforge-core-tests/prompts/valid/prologue-prose-epilog.md` - remove `default_return` from the frontmatter if present.
- Check `crates/promptforge-core-tests/src/suite/parsing.rs` for any fixture referencing `default_return`.

**Verify:** `cargo test --workspace` passes. No mention of `default_return` remains in `src/` (the research file is exempt).

## Step 2: Preserve `reply` across `jump()`

**Intent:** Stop clearing `reply` when `jump()` transfers control. The invariant becomes: when entering a section, `reply` holds the previous model output regardless of how control arrived.

**Code:**

- [`crates/promptforge-core/src/execute/engine.rs`](crates/promptforge-core/src/execute/engine.rs) line 182-183: Delete `last_reply = None;` from the `SectionFlow::Jumped` arm.
- Update the module doc comment at lines 9-10 (says "clearing it on a jump") to say "preserving it across a jump".

**Tests:**

- Search for any test asserting `reply` is nil after a jump. Update or remove it.
- Add a test: a section produces a model reply, epilog Lua does `jump("## Target")`, and the target section's prologue Lua asserts `reply` is not nil and contains the expected text.

**Verify:** `cargo test --workspace` passes.

## Step 3: Rename `last_reply` to `reply`

**Intent:** Pure rename, zero behavior change. Unify the loop-scope variable name with the section-scope name and the Lua global.

**Code:**

- [`crates/promptforge-core/src/execute/engine.rs`](crates/promptforge-core/src/execute/engine.rs): Rename `last_reply` to `reply` at lines 164, 175, 188, 199 (line numbers approximate after steps 1-2). Keep `incoming_reply` as the parameter name on `run_one_section`.

**Tests:** No new tests. Existing tests must pass unchanged.

**Verify:** `cargo check --workspace` and `cargo test --workspace` pass. `rg last_reply crates/promptforge-core/src/execute/engine.rs` returns zero matches.

## Step 4: H1-only prompts

**Intent:** Allow prompts with no H2 sections. The H1 runs its Lua and prose blocks. The model's last reply becomes the prompt output, or `"done"` when no model spoke.

**Code:**

- [`crates/promptforge-core/src/parser.rs`](crates/promptforge-core/src/parser.rs) line ~413: Delete the `section_headings.is_empty()` check that returns `Error::Parse("prompt has no ## sections")`. Allow `prompt.sections` to be an empty `Vec<Section>`.
- [`crates/promptforge-core/src/parser.rs`](crates/promptforge-core/src/parser.rs) `Prompt::entry()` (~line 436): This method assumes `sections` is non-empty. Change to return `Option<&Section>`, or guard callers. Search all call sites in the workspace with `rg "\.entry\(\)"` and update each one.
- [`crates/promptforge-core/src/execute/h1.rs`](crates/promptforge-core/src/execute/h1.rs): Add `pub(crate) reply: Option<String>` to `LiveH1State`. Populate it from the last H1 prose block's model output (the `reply` variable already tracks this in the H1 execution loop).
- [`crates/promptforge-core/src/execute.rs`](crates/promptforge-core/src/execute.rs) ~line 238: After `execute_live_h1`, before `run_sections`, add:

```rust
if prompt.sections.is_empty() {
    return Ok(h1.reply.unwrap_or_else(|| "done".to_string()));
}
```

**Tests:**

- Rewrite `no_sections_errors` in `parser/tests.rs`: an H1-only prompt now parses successfully with empty `sections`.
- Add execution test: H1-only prompt with prose returns the model's reply text.
- Add execution test: H1-only prompt with Lua `return "value"` returns that value.
- Add execution test: H1-only prompt with only Lua and no `return` falls through to `"done"`.

**Verify:** `cargo test --workspace` passes.

## Step 5: Doc sweep

**Intent:** Update every doc file to reflect all three behavioral changes, and add two worked examples for the new capabilities.

**Files to update (check each one):**

- [`promptforge.md`](promptforge.md) - remove `default_return` from frontmatter table, change "One or more H2 sections" to "Zero or more", update example
- [`guide/src/prompt-files.md`](guide/src/prompt-files.md) - remove `default_return`, update structure rules for optional H2
- [`guide/src/execution.md`](guide/src/execution.md) - remove `default_return` from fall-through chain, document reply preservation across jump, document H1-only execution path
- [`guide/src/lua.md`](guide/src/lua.md) - update `jump()` docs: reply is preserved, author clears with `reply = nil` before jumping
- [`guide/promptforge-user-guide.md`](guide/promptforge-user-guide.md) - same changes as above where duplicated
- [`crates/promptforge-core/user-guide-promptforge-core.md`](crates/promptforge-core/user-guide-promptforge-core.md) - same
- [`crates/promptforge-core/design-core.md`](crates/promptforge-core/design-core.md) - remove `default_return` from invariant 10, update execution model
- [`crates/promptforge-core/design-core-residue.md`](crates/promptforge-core/design-core-residue.md) - update mentions
- [`research/2026-08-03-recover-rationale-promptforge-core.md`](research/2026-08-03-recover-rationale-promptforge-core.md) - annotate the `default_return` rationale entries as resolved/removed
- [`guide/promptforge-report.md`](guide/promptforge-report.md) - update if it documents jump/reply behavior
- [`crates/promptforge-core-tests/prompts/valid/prologue-prose-epilog.md`](crates/promptforge-core-tests/prompts/valid/prologue-prose-epilog.md) - test fixture, remove `default_return` if not already handled in step 1

**New examples to add (use 4-tick outer fences when the example contains triple-backtick Lua blocks):**

H1-only prompt example - add to `guide/src/prompt-files.md` and `guide/src/execution.md`:

````markdown
---
name: summarize
description: Summarize the input
promptforge: 1
---

# Summarize

```lua
models.always("m", "A model suited for careful analysis")
```

Summarize this text in one paragraph.

{{ args }}
````

Reply across `jump()` example - add to `guide/src/execution.md` and `guide/src/lua.md`:

````markdown
## Analyze

Analyze this input for severity. End with exactly CRITICAL or NORMAL.

{{ args }}

```lua
if reply:find("CRITICAL") then
    jump("## Alert")
else
    jump("## Summary")
end
```

## Alert

The analysis found a critical issue:

{{ reply }}

Escalate this with recommended actions.
````

**Verify:** `cargo test --doc --workspace` passes. `rg default_return` in the workspace returns zero hits outside `research/` and `_trash/`.

## Binders

Five commits, each testable and green. Run `cargo check`, `cargo clippy -D warnings`, and `cargo test` after every commit. Do not proceed until green. Fix borrow errors by restructuring, not by clone or unsafe. Do not add dependencies. Keep each commit small.
