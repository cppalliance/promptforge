---
produced: 2026-08-03
title: rationale ledger recovering the design of promptforge-core from its code alone, forced narrowed open verdicts
---

# Rationale ledger: recovering the design of promptforge-core from its code

This is the ledger for the recovery described in the plan. It holds one record per design element. The method below is restated from the plan's sections 2 and 3 so the ledger stands on its own; a later step that disagrees with this format changes it here, in its own commit.

## The method, precisely

**What counts as a design element.** Include it only if changing it would change one of three things: what a person sees, reads, writes, types, or names - for a library that means the public API and its contracts, and it emphatically includes the *names*; or the shape of the system; or something costly to reverse that nobody sees, such as an on-disk format, a cross-cutting convention, or a security or failure-mode trade-off. A private helper, an internal algorithm, or a dependency version is implementation.

**What counts as evidence.** Anything structural in the permitted sources: a type, a signature, a lint setting, an edition, a test that would fail otherwise, a trait bound, a name, an absence where a presence would be expected. Also the language and its ecosystem - knowing that `set_var` is unsafe in edition 2024 is reasoning, not reading. Plausibility is *not* evidence. "This is the sort of thing people do for testability" kills nothing.

**How a hypothesis dies.** Either the evidence makes it impossible, or it makes it unnecessary - something else already forces the outcome, so this reason cannot be why. Record which, and cite the file and item.

**A hypothesis no evidence could ever touch is discarded, not counted.** "As a matter of taste" and "the author preferred it this way" are true of every choice ever made and can never be refuted by anything in a file. Write `unfalsifiable` where that hypothesis's evidence would go and leave it out of the survivor count. Without this rule every record carries one immortal taste hypothesis, nothing is ever forced, and the verdicts all slide one notch toward open. Discarding one is not free: a record whose only real competition was unfalsifiable has three hypotheses in name and one in fact, which the survivor rule below forbids.

**The three verdicts.**

- **Forced** - one explanation survives. State it as fact and name the constraint that forced it.
- **Narrowed** - two survive. Name both and say what would distinguish them.
- **Open** - three or more survive, or the choice is arbitrary within a range. Say the code does not determine it.

An element whose hypotheses were never seriously competing is a failure of the method, not a success: if all four candidates are variations of one idea, the collapse is theatre. Three genuinely different explanations, or say so.

## Pass 1 is blind, and the isolation is the experiment

Pass 1 may read: everything under `crates/promptforge-core/src` and its tests; `crates/promptforge-core/Cargo.toml`; the workspace `Cargo.toml`, `clippy.toml`, and `rustfmt.toml`; and the `src` of sibling crates, but only to see how core's API is *used*, since call sites are legitimate structural evidence.

Pass 1 may not read: `crates/promptforge-core/design-core.md`; any other `design-*.md` in any crate; anything in the `promptforge-design` repository; `README.md`, `STATUS.md`, or `AGENTS.md`, all of which carry rationale; and any git history - no `log`, `show`, `blame`, or commit message.

One practical guard, because the forbidden files sit next to the permitted ones and a careless search walks straight into them. Every crate keeps its design document beside its `src` directory, so an unscoped grep across `crates/` prints rationale from `design-core.md` into the very context that is supposed not to have it, and once printed it cannot be unread. Scope every search to source: `rg <pattern> --glob 'crates/*/src/**' --glob '!*.md'`, or name the file directly. A pass 1 step that has already seen a forbidden line says so in its report rather than pretending otherwise, because a contaminated record dressed as a clean one is worse than a missing one.

**Comments are treated as absent.** Every `//`, `///`, and `//!` is invisible in pass 1. This is deliberately blunt: deciding which comments merely describe and which give reasons is itself a judgement call, and pass 1 is meant to have none. Identifiers survive, including test names, which are often the best evidence in the crate.

## Record format

```
### E-0NN  headline
Element:    the API item or constant, and its file
Kind:       public API / shape / on-disk format / cross-cutting convention / trade-off
Hypotheses: H1 ...
            H2 ...
            H3 ...
            H4 ...
Evidence:   structural facts, each citing a file and item
Survives:   which hypotheses live, why the rest died (impossible or unnecessary), which are unfalsifiable
Verdict:    FORCED / NARROWED / OPEN
Reach:      how much breaks if this changes
Seen by:    whether a person ever encounters it
```

`Reach` and `Seen by` are the ranking inputs for the document: how much breaks if this changes, and whether a person ever encounters it. Both are needed, because a single configuration key can have almost no reach and still be the first thing a reader must understand.

## Records

### E-001  The gateway client is passed in, never read from the environment
Element:    RunOptions::client, crates/promptforge-core/src/execute.rs
Kind:       public API
Hypotheses: H1 injection for testability
            H2 a file-configured caller cannot set the environment
            H3 avoiding global mutable state, as taste
            H4 incidental, no reason
Evidence:   workspace Cargo.toml, [workspace.lints.rust] unsafe_code = "forbid"
            edition 2024: std::env::set_var is unsafe
            client.rs:163 GatewayClient::from_env retained as the None path
Survives:   H2. H1 is a consequence, not a cause. H3 is unfalsifiable here.
            H4 refuted by the retained fallback.
Verdict:    FORCED
Reach:      every caller of execute::run
Seen by:    the caller

### E-002  The default tool-iteration cap is 24
Element:    DEFAULT_MAX_TOOL_ITERATIONS, crates/promptforge-core/src/execute.rs
Kind:       failure-mode trade-off
Hypotheses: H1 measured, at the point where prompts stop converging
            H2 derived from a token budget
            H3 a round number chosen to be generous
            H4 inherited from somewhere else
Evidence:   execute.rs:50 constant declared
            execute.rs:231 read once, as the fallback when a prompt's frontmatter declares no max_tool_iterations
            test tool_loop_uses_the_default_cap_when_unspecified drives a model that never converges, asserts the loop makes exactly that many round trips, then asserts the constant equals 24
            no other constant relates to it; no test probes behaviour on either side of it; 24 is not a round number
Survives:   H1, H2, H4 all survive - nothing in the crate distinguishes measured, budgeted, or inherited. H3 is weighed against by 24 not being a round number, but not refuted.
Verdict:    OPEN
Reach:      every run whose prompt does not set max_tool_iterations
Seen by:    the caller, on a prompt that never converges
