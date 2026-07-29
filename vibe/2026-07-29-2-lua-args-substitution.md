---
name: Lua args substitution
overview: "Add {{ }} substitution over args, var, and sys, plus the writable var table, wired to run before the model turn. sys is a runtime-populated read-only namespace: sys.when (launch snapshot), sys.now (build-time snapshot), sys.id (per-context id). (mlua embedding, single-string args, and finish-on-return are already built.)"
todos:
  - id: subst-sys-var
    content: "subst.rs: resolve {{ args }}, {{ var.x }}, {{ sys.x }} in prose (scalar->string, table->JSON, missing->error, single pass). var writable table. sys namespace (when/now/id) populated by the runtime. Wire: run Lua (sets var) -> substitute -> model turn. prompts/greet.md demo + unit tests + e2e. Commit."
    status: pending
isProject: false
---

# PromptForge: substitution + var + sys

## Already implemented (do not redo)

Committed in the Lua echo commit: mlua embedded and sandboxed
(`promptforge-core::lua::run_chunk`), the single raw `args` string exposed to a
section's Lua block, and the finish case of the exit rule (a chunk that returns a
plain value ends the run with it). `execute::run(prompt, args)` runs the entry
section's Lua and, on a returned value, finishes; otherwise it sends the prose to
the gateway. CLI: `promptforge run <file> [input]`.

## Remaining: substitution + var + sys

Add the `{{ }}` substitution layer, the writable `var` table, and the
runtime-provided `sys` namespace, so Lua-computed and runtime values flow into
the prose the model sees. This all lands on the existing "no return -> model"
branch of `execute::run`.

- `crates/promptforge-core/src/subst.rs` (new) - resolve `{{ ns.path }}` in prose for namespaces `args`, `var`, `sys`: scalar -> string, table -> JSON (serde_json), missing key -> hard error, single pass, no recursion. Add `Error::Substitution(String)`.
- `var` - a writable table the Lua block populates; read back after the chunk runs.
- `sys` - runtime-populated, read-only, available to both substitution and Lua:
  - `sys.when` - launch timestamp, captured once at run start (fixed for the run).
  - `sys.now` - a build-time snapshot of the current time (see deferred note on live time).
  - `sys.id` - a per-context id, 1-based, assigned deterministically by context-creation order.
- Wire in `execute.rs`: populate `sys`, run the Lua block (which sets `var`), substitute the prose using `args` + `var` + `sys`, then the model turn.
- `prompts/greet.md`: `## Main`, Lua `var.greeting = "Hello, " .. args .. "!"`, prose `Repeat exactly, no extra words: {{ var.greeting }}`. The model echoes the substituted prose.

## Deferred (not this increment)

- **Live time across turns** - `sys.live("now")` (runtime refreshes a single `now` in the replace-in-place floated tail each turn) and/or a `now()` tool. `sys.now` here is only the build-time snapshot.
- The `facts` bag, and all control flow (fall-through, goto/task/fanout, the tool-call loop).

## Design notes

- `args` is a single raw string (caller input); derived structure is deduced+stored by the pipeline in later rungs.
- Substitution is pure path lookup - no formulas. Computation happens in Lua (`var.c = var.a + var.b`), then `{{ var.c }}`.
- `sys` names provenance (system-provided), not immutability, so a fixed `when` and a snapshot `now` coexist under it.

## Tests

- `subst` unit: `{{ args }}`; `{{ var.x }}` after Lua sets it; `{{ sys.id }}` and `{{ sys.when }}`; a `var` table field -> JSON; missing key -> error; text with no placeholders unchanged.
- e2e (manual, through the gateway): `promptforge run prompts/greet.md "World"` prints `Hello, World!`.

## Verification

`cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` all green; manual gateway run prints the greeting. Update STATUS.md; commit.
