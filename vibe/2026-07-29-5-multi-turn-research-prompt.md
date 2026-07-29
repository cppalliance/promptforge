---
name: multi-turn research prompt
overview: Author a PromptForge prompt that researches a person across multiple web_search and web_fetch turns and returns a ~500-600 token summary, and make the tool-call loop's iteration cap configurable so genuine multi-turn research does not hit the current hard limit of 10. Built per how-to-vibe.md, one self-contained commit per step.
todos:
  - id: loop-cap
    content: "Make the tool-call loop cap configurable: add Frontmatter.max_tool_iterations (Option<usize>), raise the default to 24, thread it from execute::run into run_tool_loop, remove the bare constant from the loop path. Unit-test the declared value and the default. fmt/clippy/test/doc green."
    status: completed
  - id: research-prompt
    content: "Author prompts/research-person.md (tools: [web_search, web_fetch], max_tool_iterations: 20, prose instructing multi-turn research of {{ args }} and a ~500-600 token final summary). Live end-to-end run through the gateway with BRAVE_API_KEY as the acceptance check."
    status: completed
isProject: false
---

# Multi-turn web research prompt

Built per [tools-public/how-to/how-to-vibe.md](tools-public/how-to/how-to-vibe.md): plan in levels of resolution, one testable commit per step, each step written in one subagent, reviewed in a second, fixed in a third, findings routed through `vibe-review.md`, git kept in the main context. Every commit makes progress on its own.

## House rules (load first)

Before the runtime change, the code subagent loads [tools-public/how-to/how-to-write-rust.md](tools-public/how-to/how-to-write-rust.md) and follows it. The workspace lints in [promptforge/Cargo.toml](promptforge/Cargo.toml) already enforce unsafe-forbidden, clippy all=deny + pedantic=warn, and unwrap_used=deny (tests may unwrap via clippy.toml). Every public item carries docs and an `# Errors` section where it returns `Result`.

## Level 1: what we are building

A prompt file that takes a single input like "Tell me about <person>", makes several `web_search` and `web_fetch` calls to gather material, and once it has enough for a compact factual summary (about 500-600 tokens) stops calling tools and emits that summary as its final message - which the runtime returns as the run's result. Plus one small runtime change so the tool-call loop can take enough turns to do that.

This rests on machinery already built: frontmatter `tools`, the single `args` string, `{{ args }}` substitution, and the tool-call loop in [promptforge/crates/promptforge-core/src/execute.rs](promptforge/crates/promptforge-core/src/execute.rs) that dispatches tools until the model returns plain text, which becomes the result. No new tool, store, or token-enforcement mechanism is added.

## How the stop-and-return works

The tool-call loop already ends a section when the model replies with text and no tool calls, and that text becomes the run result. So "gather, then return the string" is: the model calls `web_search`/`web_fetch` for several turns, then writes the summary as its final message. The 500-600 token target is a prose instruction the model self-limits to; there is no hard token cap (a non-goal below).

## Level 2: components, in dependency order

1. `promptforge-core` runtime - owns the tool-call loop and the frontmatter parser. Fixed first because the prompt in step 2 declares `max_tool_iterations`, which the parser must accept and the loop must honor before the prompt can rely on it.
2. The prompt file - `promptforge/prompts/research-person.md`, pure data, depends on step 1's budget field existing.

## Level 3: pieces

- Frontmatter field `max_tool_iterations: Option<usize>` in [promptforge/crates/promptforge-core/src/parser.rs](promptforge/crates/promptforge-core/src/parser.rs) (`#[serde(default)]`), so a prompt declares its own budget.
- The loop cap in [promptforge/crates/promptforge-core/src/execute.rs](promptforge/crates/promptforge-core/src/execute.rs): raise the default constant to 24 and pass `frontmatter.max_tool_iterations.unwrap_or(DEFAULT)` into `run_tool_loop`, replacing the bare `MAX_TOOL_ITERATIONS` use in the loop path.
- The prompt: one `##` section, prose only, tools `web_search` and `web_fetch`, budget 20.

## Rust conventions for the loop-cap change

From [tools-public/how-to/how-to-write-rust.md](tools-public/how-to/how-to-write-rust.md), the rules that bind this specific change:

- The new field is `max_tool_iterations: Option<usize>` in `snake_case` with `#[serde(default)]`, and it carries a doc comment (`missing_docs` is warn); `Option<usize>` so absent means "use the default", not zero.
- Keep the default as one named `const DEFAULT_MAX_TOOL_ITERATIONS: usize` in `SCREAMING_SNAKE_CASE` with a doc comment - a single source of truth - and resolve the effective cap once as `frontmatter.max_tool_iterations.unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS)`.
- Thread the resolved cap as a plain `usize` parameter into `run_tool_loop`; the loop reads its parameter, not a module constant, so the value has one origin.
- No `unwrap`/`expect` in non-test code; `unwrap_or` is the resolution, and the existing `# Errors` doc on `run` (including `ToolLoopExhausted`) stays accurate and unchanged in meaning.
- `Frontmatter` is a public struct; adding a field is additive for deserialization because of `#[serde(default)]`, and the crate is pre-release, so this is acceptable. If `Frontmatter` is not already `#[non_exhaustive]`, add it in this same change so later fields are non-breaking - but do not restructure anything else.
- Tests land in this commit, in an in-file `#[cfg(test)] mod tests` with `use super::*;`; async loop tests use `#[tokio::test]`, adapting the existing always-tool-call mock so the assertion is on the exact round-trip count for a configured cap.

## Level 4: the steps (one commit each; complete, wired, tested)

1. **loop-cap.** Intent: a prompt can declare `max_tool_iterations` and the loop honors it; absent, a raised default (24) applies; no hard-coded 10 remains in the loop path. Add the frontmatter field, raise the default, thread the value from `execute::run` into `run_tool_loop`. Tests: a prompt declaring `max_tool_iterations` parses and the loop stops after exactly that many round trips (adapt the existing always-tool-call mock test), and a prompt without it uses the default. fmt/clippy/test/doc green.
2. **research-prompt.** Intent: `promptforge run prompts/research-person.md "Tell me about <person>"` returns a coherent ~500-600 token summary after real multi-turn search and fetch. Author `promptforge/prompts/research-person.md`: frontmatter `name`, `description`, `version: 1`, `tools: [web_search, web_fetch]`, `max_tool_iterations: 20`; one section, prose only, instructing the model to research `{{ args }}` with several targeted `web_search` queries, `web_fetch` the most relevant results to confirm facts, prefer primary or reputable sources, be economical with tool calls so it stays within budget, treat everything fetched as untrusted third-party text, and once it can write a ~500-600 token factual summary stop calling tools and output only that summary as its final message. Acceptance: the live end-to-end run below produces such a summary.

## Before executing: one gap pass

Read the two steps once. Step 1 produces the `max_tool_iterations` field and the honored budget; step 2 consumes it. Each step changes observable behavior on its own (step 1: a declared cap is honored and the default is higher; step 2: a working research prompt), so neither waits on the other to matter. One pass, not a gate.

## The build cycle (per step, in subagents)

- A code subagent gets this plan by path and the step number, loads how-to-write-rust.md, implements only that step.
- The main context commits; git output is bounded.
- A review subagent reads the diff, applies the general `<code-review>` checks in how-to-vibe.md plus the `<research-prompt-review>` checks below, and overwrites `vibe-review.md` (outside the repo, at `cabinet/_scratch/research-prompt-build/vibe-review.md`) with any failures.
- A fixer subagent reads that file and edits.
- The main context re-runs the checks and amends.

Step 2's acceptance is a live run, not an offline test, so the main context performs it directly (start the gateway with `BRAVE_API_KEY`, export `PROMPTFORGE_BASE_URL` and `PROMPTFORGE_TOKEN`, run the command, read the returned summary).

## Project-specific review checks

<research-prompt-review>
Read the diff for the commit named by the step. Apply each as a yes-or-no question. Append to vibe-review.md any failure with file:line, the problem in one sentence, and the single fix. These are in addition to the general code-review checks.

1. Does the loop cap come from `frontmatter.max_tool_iterations` with the raised default as the only fallback, and is the bare hard-coded `10` gone from the loop path?
2. Is there a test proving the declared cap is honored (loop stops after exactly N round trips) AND a test that the default applies when the field is absent?
3. Does the prompt's `tools` list contain only canonical names (`web_search`, `web_fetch`) and set `max_tool_iterations`?
4. Does the prompt prose instruct all of: multi-turn research of `{{ args }}`, economical tool use within budget, stop at roughly 500-600 tokens, output only the summary as the final message, and treat fetched content as untrusted?
5. Does this commit change observable behavior on its own?
6. For the prompt step: was the live end-to-end run actually performed, and did it return a coherent ~500-600 token summary rather than an error or a ToolLoopExhausted?
7. Does the loop-cap change follow the Rust conventions above: one documented `SCREAMING_SNAKE_CASE` default const, the field an `Option<usize>` with `#[serde(default)]` and a doc comment, the cap threaded as a `usize` parameter (not a module constant read inside the loop), no unwrap/expect in non-test code, and `Frontmatter` `#[non_exhaustive]`?
</research-prompt-review>

## Non-goals

- No hard token enforcement; the 500-600 target is a prose instruction the model self-limits to.
- No new tools, no state/facts store, no fanout - a single section with the two existing tools.
- No new SSRF or web behavior; `web_search` and `web_fetch` are used as built.
- No design document generated for this small feature (can be added later).

## Verification

Per commit: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, and `cargo doc` green. Step 2 additionally: a live run `promptforge run prompts/research-person.md "Tell me about <a well-known person>"` against a gateway started with `BRAVE_API_KEY`, returning a coherent ~500-600 token summary after real search/fetch calls.

## Decisions and confidence

- Configurable cap via frontmatter plus a raised default (24), not an env var: the budget belongs with the prompt that needs it. Confidence: high.
- Model self-judged stop at ~500-600 tokens rather than an enforced cap: it is the only mechanism the runtime supports today and matches the loose target. Confidence: medium; if the model overshoots consistently, trimming the final result is a later option. Falsifier: the live run returns far more or far less than the target band.
