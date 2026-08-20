# Silent Tool-Loop Exits and Self-Loading Env Files for PromptForge

## Executive summary

Two behaviors changed. First, the prose tool loop in promptforge-core now accepts an empty final turn as a clean exit: when the model returns no tool calls and no text with `finish_reason == "stop"`, and at least one tool call was successfully dispatched earlier in the loop, the run succeeds and the section's `reply` binds to `""`. Every other empty turn still fails as `EmptyModelReply`. This is default behavior with no opt-in flag, and it exists because a prompt that records all of its work through tool calls and then says nothing is a legitimate, deliberately authored pattern that the old invariant made impossible. Second, the MCP server now loads the env file name-matched to its configuration (`prompts.env` beside `prompts.toml`) into an in-memory map that supplies `${VAR}` interpolation defaults behind the process environment, matching the gateway's observable precedence without mutating the process environment. A missing or malformed env file never fails a load. Operators no longer source env files by hand before starting the server.

## Key design choices

1. **An empty stop-turn after successful tool work is a clean exit, not an error.** What the user sees: a prompt whose final model turn produces no text and no tool calls now succeeds instead of failing the run, and the section's `reply` is the empty string. The motivation is that models legitimately end a tool loop with `content: ""` and `finish_reason: "stop"` once they have recorded everything through tool calls; prompts written as "record everything via tools, output nothing" are a real authoring pattern, and the old rule forced such prompts to emit filler text purely to satisfy the runtime. Acceptance is unconditional default behavior rather than an opt-in flag because the gate is narrow enough that no genuine failure is masked by it. Reversing this later would break every prompt that has come to rely on silent exits, so the contract is effectively permanent once released.

2. **The acceptance gate fails closed on three conditions.** The exit is clean only when the turn carries no tool calls and no text, the wire `finish_reason` is exactly `"stop"`, and at least one tool call was successfully dispatched earlier in the same loop. A missing finish reason, a `"length"` or any other reason, or an empty turn with zero prior dispatches all remain `EmptyModelReply` failures. The rationale is that each condition excludes a distinct failure mode: truncation means the model's answer was cut off and work may be lost, a missing reason is unclassifiable and therefore unsafe to assume deliberate, and an empty first turn means the model did nothing at all, which is precisely the glitch the original invariant exists to catch.

3. **Normalization always raises; the loop decides.** An empty turn is still raised as `Error::EmptyModelReply` at the normalization layer in every dialect, with the choice's `finish_reason` now carried on the error value, and the tool loop alone decides whether to accept it. This keeps the normalization layer model-agnostic and free of loop policy: it reports the fact of an empty product plus the evidence needed to classify it, and the one place that owns loop semantics applies the policy. The error variant is `#[non_exhaustive]`, so adding the field changed no existing match site.

4. **The finish reason is part of the public error surface.** `CompletionError` exposes a `finish_reason()` accessor that answers the wire reason when the failure was an empty reply. Callers outside the loop can now distinguish a deliberate stop from a truncation or a transport glitch without parsing display strings. The cost of this choice is a permanent public API commitment; the alternative, keeping the reason internal, would have forced every external classifier to scrape error text, which is a worse contract to be stuck with.

5. **The accepted exit is observably a completed turn.** The accepted turn runs the same post-turn bookkeeping as a text-reply exit: observers receive `Model turn completed`, turn totals include it, and `sys.reply_finish_reason` still publishes `"stop"` to the Lua side. A silent exit must not look like a vanished turn, or turn-cap accounting and run telemetry would drift from what the model actually did. No debug capture fires for the accepted turn because the failed completion carries no request or response bodies to record; there is nothing to capture.

6. **"Successful tool call" means completed dispatch, never attempt.** The counter increments only after a tool call is dispatched successfully, and a tool handler failure aborts the loop outright, so reaching a later round already proves every earlier call completed. The gate therefore certifies observable side effects: the model did work that actually landed, not work it merely attempted. Counting attempts would let a loop whose tools all failed exit silently, which is exactly the failure the operator needs to see.

7. **Single-shot prose needs no clause, and says so.** The acceptance conditions can never hold in single-shot mode because its only round is the first turn, where the dispatch count is always zero. The code carries a comment stating this rather than a dead branch, so a future reader does not "fix" the asymmetry by adding a clause that cannot fire or, worse, one that weakens the first-turn invariant.

8. **A silent exit binds the empty string, not a sentinel.** Lua sees `reply == ""`, indistinguishable from any other empty binding, rather than a placeholder like `"(no output)"`. A prompt that ends silently has nothing to say; inventing text would put words in the model's mouth, would show up in downstream string comparisons and serialized output, and would be a second contract to support forever.

9. **The env file is name-matched to the configuration, by convention alone.** The MCP server resolves `prompts.env` beside `prompts.toml` by replacing the extension, the same convention the gateway uses. What the operator sees is one rule: the env file travels with the config file, with no setting to name it and therefore no setting to mispoint. A configurable path was the alternative and lost, because it adds a knob whose only effect is to let the two files disagree.

10. **The process environment wins; the file supplies defaults.** Lookup order is the process environment first, then the env-file map, which reproduces the precedence the gateway gets from dotenvy's no-override semantics. The observable consequence is that an operator can override any file value for a single launch by exporting it, without editing the file, and the two servers behave identically so there is one mental model for both.

11. **Values live in an in-memory map, never in the process environment.** The env file is parsed into a map threaded into interpolation through a lookup closure, rather than loaded via dotenvy's process-env mutation, because `std::env::set_var` is `unsafe` under edition 2024 and this crate forbids unsafe. The map reproduces the gateway's observable behavior, lookup order included, without the unsafe. Two further properties fall out of the structure: file values never leak into the process environment where child processes or unrelated code could read them, and tests inject the lookup closure outright instead of mutating real environment state. Reversing this would cost an explicit unsafe exception to the crate's policy and would buy nothing observable.

12. **The env file can never fail a load.** A missing file is skipped silently and a malformed one is ignored with a warning, so a configuration that loaded yesterday cannot break today because an optional companion file is absent or damaged; this mirrors the gateway, which never fails a load over the env file either. One secret-hygiene detail is load-bearing: the parse error's display text embeds the offending line, which can carry a secret value, so only the line index reaches the log, never the line itself.

13. **Reload re-reads the env file, but saving it alone triggers nothing.** The map is rebuilt on every configuration load, so any reload that re-reads the TOML also picks up env-file changes. The file watcher answers only to the configuration file and the prompts directory, so an env-only edit takes effect on the next config touch or restart. Keeping one watch surface was chosen over watching the env file too, because a half-applied credential change (new file values against an old config) is a more confusing state than a slightly delayed one.

## Only one empty-turn shape exits cleanly

The full classification contract, which the tests pin:

| Turn text | Tool calls in turn | `finish_reason` | Prior successful dispatches | Outcome |
|---|---|---|---|---|
| empty | none | `"stop"` | one or more | clean exit, `reply` binds `""`, turn observed as completed |
| empty | none | `"stop"` | zero | `EmptyModelReply` |
| empty | none | `"length"`, other, or missing | any | `EmptyModelReply` |
| non-empty, or tool calls present | any | any | any | normal processing, unchanged |

## Interpolation precedence is the gateway's precedence

`${VAR}` in the MCP server's TOML resolves from the process environment first and the name-matched env file second; `$$` remains a literal dollar; an unset variable still fails the load everywhere except `[server].api_key`, where the key drops silently so a stdio install can boot without a credential its transport never reads. The env file widens where values come from and changes nothing about what an unset variable does.

*2026-08-18 - kimi-k3*
