---
name: Empty-turn policy and MCP env loading
overview: "Two fixes from the papergate debugging session: (1) accept an empty final tool-loop turn as a clean exit when finish_reason is \"stop\" and at least one tool call succeeded, and (2) make promptforge-mcp-server load its name-matched .env file before interpolation, matching the gateway."
todos:
  - id: core-error-field
    content: Add finish_reason to Error::EmptyModelReply and thread it through normalize.rs, the gemma dialect, and the CompletionError conversion
    status: completed
  - id: core-loop-exit
    content: "tool_loop.rs: track successful tool calls, accept empty stop-turn as clean exit with empty reply"
    status: completed
  - id: core-tests
    content: Update and add promptforge-core tests for the new exit policy
    status: completed
  - id: core-docs
    content: Update design-core.md and guide docs for the refined invariant (folded into the code commits, not a standalone commit)
    status: completed
  - id: mcp-env-load
    content: "MCP server: parse name-matched .env into a map, thread lookup through interpolate_document"
    status: completed
  - id: mcp-tests
    content: Add env-file loading and precedence tests in config/tests.rs
    status: completed
  - id: mcp-docs
    content: Update design-mcp-server.md env section
    status: completed
  - id: e2e-verify
    content: Run cargo tests, then end-to-end papergate run with reverted prompt and bare server start
    status: completed
isProject: false
---

# Empty-Turn Policy + MCP Env Loading

## Context

The papergate run failed because the prompt said "Do not output any text": the model recorded all sections via tool calls, then legally exited the loop with `content: ""`, `finish_reason: "stop"`, which `normalize.rs` hard-fails as `EmptyModelReply`. Separately, the MCP server resolves `${VAR}` only from the process environment (`std::env::var`), while the gateway loads name-matched `.env` files via dotenvy - forcing manual env sourcing at startup.

User decisions: empty-turn acceptance is **default behavior** (no opt-in flag); empty exit yields `reply = ""` (empty string).

## Part 1: Accept empty final turn after successful tool calls

Policy: in the prose tool loop, a turn with no tool calls and empty text is a clean loop exit (reply = `""`) when `finish_reason == "stop"` AND at least one tool call was successfully dispatched earlier in the loop. All other empty turns (no prior tool calls, or `finish_reason` missing/`"length"`/other) remain `EmptyModelReply` errors. Tool handler failures already abort the loop, so any completed dispatch counts as a success.

### Changes in `crates/promptforge-core`

1. **`src/error.rs`** - add `finish_reason: Option<String>` field to `Error::EmptyModelReply` (currently only `detail: &'static str`, ~line 170; the variant is `#[non_exhaustive]`, so this is not breaking). Update constructor usages.

2. **`src/normalize.rs`** - `empty_reply_error()` (line 119) takes the `finish_reason` from `TurnContext` (already bound at the raise site, lines 131-181) and stores it on the error. Update module docs (lines 1-7) to note that the error carries the choice's `finish_reason` for tool-loop classification. (Revised by the `core-error-field` commit: the refined invariant - empty product is an error *unless* the tool loop accepts it as a stop-exit - moves to the `core-loop-exit` step, because the acceptance behavior does not exist yet and documenting it here would describe unreal behavior.)

3. **`src/dialects/gemma3_tool_code/mod.rs`** (lines 171-173) - pass `finish_reason` through at its `empty_reply_error` call site; verify the field is in scope there.

4. **`src/model/error.rs` + `src/client/transport.rs`** - the loop never sees `Error::EmptyModelReply` directly: `GatewayClient::complete` converts it to `CompletionError` (kind `EmptyReply`, `model/error.rs:79`) at `transport.rs:277-284`. Carry `finish_reason` through that conversion so the loop can gate on it.

5. **`src/execute/tool_loop.rs`** - the core change:
   - Track `successful_tool_calls: usize` in `run_prose_inference`, incremented after each successful dispatch (local tools lines 255-267, registry tools lines 276-293).
   - At the `client.complete` call (lines 173-185), catch the empty-reply `CompletionError` instead of unconditionally propagating: if `finish_reason == Some("stop")` and `successful_tool_calls > 0`, run the normal post-turn bookkeeping for the accepted turn (`advance_turn`, `MODEL_TURN_COMPLETED` at lines 187-210, so observers and turn counts see it), then return `ProseInferenceResult { text: Some(String::new()), finish_reason }` (clean exit, empty reply). Otherwise observe `MODEL_TURN_FAILED` and propagate as today.
   - `ProseMode::SingleShot`: unchanged. The acceptance conditions cannot hold on a first turn (no prior tool calls), and single-shot never re-enters after dispatch, so no clause is needed; add a code comment saying so.
   - `run_tool_loop` (lines 112-115) needs no change - `Some("")` flows through, and `engine.rs:586` binds it to `reply`.

6. **`src/execute/engine.rs`** - verify `bind_reply` with `""` is harmless (it binds the empty string to Lua `reply`; `sys.reply_finish_reason` enrichment at lines 579-583 already handles the finish reason).

### Tests (update + new)

- Update `empty_final_text_fails_the_turn` (`src/execute/tests/mod.rs:920`) - still fails: no prior tool calls.
- `empty_truncated_final_text_fails_without_truncation_detail` (`mod.rs:955`) - still fails: `finish_reason: "length"`.
- New: tool call succeeds, then empty `stop` turn -> loop exits, section reply is `""`, run succeeds, and the accepted turn is observed (`MODEL_TURN_COMPLETED`, turn count includes it).
- New: empty `stop` turn with zero prior tool calls -> `EmptyModelReply`.
- New: empty turn with missing `finish_reason` after tool calls -> still `EmptyModelReply` (fail closed).
- Update `normalize.rs` unit tests for the new error field.

### Docs

- `crates/promptforge-core/design-core.md` - revise item 25 (line 84), step 6 (line 111), and the observation paragraph (line 122) to state the stop-exit exception.
- `guide/src/prompt-files.md` / `guide/src/lua.md` - document that a tool loop may end silently and `reply` is then `""`.

## Part 2: MCP server loads name-matched .env file

Mirror the gateway (`crates/promptforge-gateway/src/profile.rs:119-162`: `config_path.with_extension("env")`, missing file silently skipped) but **without** dotenvy's process-env mutation - `design-mcp-server.md:70-72` forbids `unsafe`, and `std::env::set_var` is unsafe under edition 2024. Instead, parse the env file into an in-memory map and thread it into interpolation. Precedence matches gateway behavior: process environment wins, env file supplies defaults (gateway gets this from dotenvy's no-override semantics).

### Changes in `crates/promptforge-mcp-server`

1. **`src/config.rs`**:
   - `Config::load(path)` (lines 339-397): after reading the TOML, check `path.with_extension("env")`; if it exists, parse it into a `BTreeMap<String, String>` (reuse `dotenvy::from_path_iter` - iterator-only, no `set_var`, no unsafe; add `dotenvy.workspace = true` to the crate's Cargo.toml). A missing env file is silently skipped, and a malformed one is ignored with a `tracing::warn` - parity with the gateway's `let _ = dotenvy::from_path(...)`, which never fails the load over the env file.
   - Refactor `from_toml_str` to take a lookup: keep `from_toml_str(&str)` delegating to a new private `from_toml_str_with(&str, &dyn Fn(&str) -> Option<String>)` that passes the lookup into `interpolate_document`. Lookup order: `std::env::var` first, then the env-file map (mirrors gateway precedence).

2. **`src/config/interpolate.rs`** - `interpolate_document` (line 19) and `interpolate` (line 56) accept the lookup closure instead of hardcoding `std::env::var`; `interpolate_with` (line 61) already has the right shape.

3. Hot reload (`src/watch/reload.rs` ~line 304) goes through `Config::load`, so it picks up env-file changes automatically - no change needed.

### Tests (`src/config/tests.rs`)

- New: `.env` file beside `.toml` resolves `${VAR}` (tempdir, `Config::load`).
- New: process env beats env-file value - tested through the injected lookup with a stub map standing in for the process env, never `std::env::set_var`, which is `unsafe` under edition 2024 and forbidden in this workspace.
- New: missing env file loads fine when all vars resolve from process env.
- New: malformed env file is ignored (load succeeds on process-env vars) and warns.
- New: env-file value does not leak into process env (`std::env::var` still unset after load).
- Existing interpolation tests keep passing through the process-env-only default lookup.

### Docs

- `crates/promptforge-mcp-server/design-mcp-server.md` - add a section documenting name-matched env-file loading and why it uses an in-memory map (no unsafe env mutation).
- `guide/src/mcp-server.md:25` already claims one name-matched `.env` file - verify wording matches implemented precedence.
- `promptforge.md` quickref - no change needed (already describes the intended behavior); optionally note the manual `set -a; . file.env` workaround is gone.

## Verification

1. `cargo test -p promptforge-core -p promptforge-mcp-server`
2. End-to-end: revert `local/prompts/papergate.md` line 44 to "Do not output any text.", restart both servers (no env sourcing), run `promptforge papergate` on the P4222R1 inbox file - should succeed and produce the section list.
3. Kill servers, unset any exported vars, start MCP server bare - should come up resolving keys from `local/mcp-service.env`.

## Execution rules

This plan runs under the vibe-rulebook (c:\Users\Vinnie\src\cursor\tools-public\rulebooks\vibe-rulebook.md) and the rust-rulebook (c:\Users\Vinnie\src\cursor\tools-public\rulebooks\rust-rulebook.md). Dispatch subagents with these paths so they can load the rules themselves.

- Work the todos as testable commits. Part 1 (`promptforge-core`) and Part 2 (`promptforge-mcp-server`) touch disjoint crates and may run as two parallel workstreams; within each part, keep step order. Each step is one commit carrying its code, its test, and its documentation. Per step: dispatch the coder subagent, commit, dispatch the review-and-fix subagent against the diff (applying `<code-review>` below, overwriting `vibe-review.md`), amend if review dirtied the tree. Run Verify (build + tests) on every 3rd step, at the end of each crate's work, and on the final step.
- Do all search, read, edit, review, and test work in subagents; keep this session's context clean. Pass results through scratch files.
- Rust conventions per the rust-rulebook: `Result` for expected failures, `#[non_exhaustive]` on public error variants carrying data, doc every public item with `# Errors` where it applies, test in the same change as the code, and `cargo fmt --all --check` plus `cargo clippy --all-targets --all-features -- -D warnings` before every commit.
- Decision record: where an implementation step contradicts, extends, or resolves a decision this plan records, that step revises the plan in the same commit, naming what forced the change.

## Final step: generate the design document

After implementation is complete, generate the design document: spawn one subagent whose entire prompt is - read this plan at c:\Users\Vinnie\.cursor\plans\empty-turn_policy_and_mcp_env_loading_ff979c47.plan.md, grep for `<design-doc>`, and follow the block inside it.

<design-doc>
OUTPUT A DESIGN DOCUMENT, NOT CODE. Write one markdown file, design-empty-turn-exit-and-env-loading.md,
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

## Out of scope (noted, not fixed)

- Outbound history serializes assistant tool turns as `"content": ""` (`client/wire.rs:81-88`); Anthropic 400s on empty text blocks in requests in some paths, but the live run proved this shape works through the gateway today. Revisit only if it bites.
