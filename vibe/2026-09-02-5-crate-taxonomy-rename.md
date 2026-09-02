---
name: crate taxonomy rename
overview: Delete three liability crates (mcp-server and dev outright, core-tests dissolved into core), rename every promptforge-* crate to the three-product taxonomy (gateway / promptforge library / workshop) plus the shared- and build- prefixes, and create the `promptforge` facade crate exposing `pipeline::run` and `agent::run`. 9 commits, each green. Step 8 audits every AGENTS.md for lines that earn their keep.
todos:
  - id: step-01
    content: Delete promptforge-mcp-server and promptforge-dev (unused; recoverable from git history)
    status: completed
  - id: step-02
    content: "Dissolve promptforge-core-tests: offline fixture suite moves into promptforge-core/tests/, scenario harness deleted"
    status: completed
  - id: step-03
    content: "gateway product: rename 11 crates leaf-first, binaries, config-ui npm name"
    status: completed
  - id: step-04
    content: "library + shared + tooling: workshop-agent -> promptforge-agent, promptforge-progress -> shared-progress, ui-build -> build-ui, llama-cuda-build -> build-llama-cuda, make-user-guide -> build-user-guide"
    status: completed
  - id: step-05
    content: "workshop product: promptforge-workshop -> workshop, promptforge-workshop-server -> workshop-server, ui npm name, name assertion"
    status: completed
  - id: step-06
    content: CI workflows, build-user-guide crate list, repo-level docs, guide regeneration
    status: completed
  - id: step-07
    content: "Create the promptforge facade crate: pipeline::run + agent::run re-exports, no logic"
    status: completed
  - id: step-08
    content: "AGENTS.md audit: every line earns its keep (charter/constraint/exception), update renamed-crate references, root gains taxonomy line"
    status: completed
  - id: step-09
    content: "Final sweep: full grep matrix, guide regen, full suite green (skip if empty)"
    status: completed
isProject: false
---

# Crate taxonomy rename

## For a fresh session: context in brief

- **Status: complete.** All 9 todos done. Commits: `e784dc75` `74e9c401` `964ea20f` `b19ed68c` `9b6d8e3f` (step 6 no-op) `ac3babf3` `264ee42f` `4796165c`. Open findings: none.
- Repo: `C:\Users\Vinnie\cursor\promptforge` - a Rust workspace, 32 crates under `crates/*` (verified 2026-09-02), plus two TypeScript UIs (`crates/promptforge-workshop-server/ui`, `crates/promptforge-gateway-config-ui/ui`).
- **This plan runs ONLY AFTER the agentic-harness plan is finished.** Hard precondition: all 16 steps of [interactive_webhook_tool_9ab3c21f.plan.md](c:\Users\Vinnie\.cursor\plans\interactive_webhook_tool_9ab3c21f.plan.md) are complete (its frontmatter todos all marked completed) and its run has ended. Never run the two concurrently - the harness plan's later steps touch the workshop server and the agent crate, and a mid-run rename would collide with its step 12. At completion the tree contains `workshop-agent` (its step 7), the workshop-server dependency on it (its step 12), and `crates/promptforge-workshop-server/agents/chat.lua` (its step 15); this plan's mechanical sweep renames all of it.
- Execution runs under `tools-public/rulebooks/vibe-rulebook.md`: one step = one commit with its verification; subagent dispatch prompts follow `tools-public/rulebooks/prompts-rulebook.md`; code follows `tools-public/rulebooks/rust-rulebook.md`. Ledger: `cabinet/_scratch/vibe-crate-rename/vibe-ledger.md`. Review file: `cabinet/_scratch/vibe-crate-rename/vibe-review.md`. Dirty worktree at start = stop and ask.
- Verify commands: `cargo test --workspace --locked` at the repo root (the full suite - renames touch every crate, so focused testing is meaningless); `npm run typecheck && npm test` in each UI directory only if that step touched it (none do); `cargo run -p build-user-guide` after any step that changes guide inputs (steps 1, 6, 9) to regenerate the guide.

## The taxonomy and the rename map

Three products ship after step 1 deletes the unused mcp-server product (recoverable from git history if it ever returns). Crate names encode membership. The `promptforge-` prefix survives ONLY on the library product (it IS the product); gateway and workshop crates drop it. A fourth prefix, `shared-`, marks crates that ship in more than one deliverable - no product may claim them. A fifth prefix, `build-`, marks build-time and CI tooling - it runs at compile time or in CI and is linked into no deliverable. Binaries rename to match their product.

### gateway product (binary: `gateway.exe`)

| old | new |
|---|---|
| promptforge-gateway | gateway |
| promptforge-gateway-config | gateway-config |
| promptforge-gateway-config-ui | gateway-config-ui |
| promptforge-gateway-local | gateway-local |
| promptforge-gateway-loopback | gateway-loopback |
| promptforge-gateway-protocol | gateway-protocol |
| promptforge-gateway-routing | gateway-routing |
| promptforge-stt | gateway-stt |
| promptforge-transcribe | gateway-transcribe |
| promptforge-web-search-service | gateway-web-search |
| whisper-ffi | gateway-whisper-ffi |

### promptforge library product (prefix kept)

| old | new |
|---|---|
| promptforge-core | promptforge-core (unchanged) |
| promptforge-core-support | promptforge-core-support (unchanged) |
| promptforge-lua | promptforge-lua (unchanged) |
| promptforge-model-client | promptforge-model-client (unchanged) |
| promptforge-parser | promptforge-parser (unchanged) |
| promptforge-store | promptforge-store (unchanged) |
| promptforge-tools | promptforge-tools (unchanged) |
| promptforge-tool-picker | promptforge-tool-picker (unchanged) |
| promptforge-webfetch | promptforge-webfetch (unchanged) |
| promptforge-web-search | promptforge-web-search (unchanged) |
| promptforge-cli | promptforge-cli (unchanged; binary already `promptforge`) |
| workshop-agent | promptforge-agent (joins the library product: cloud/headless agent hosts are anticipated, so the executor rides with the library rather than promoting out of the workshop later) |

`promptforge-core-tests` is not renamed - step 2 dissolves it into `promptforge-core/tests/`. `promptforge-dev` is not renamed - step 1 deletes it (unused).

The library also gains one NEW crate in step 7: the `promptforge` facade, the integrator-facing surface - `promptforge::pipeline::run` for document prompts, `promptforge::agent::run` for agent programs. No separate `promptforge-lib`; the facade carries the brand name.

### workshop product (binaries: `workshop.exe`, `workshop-server.exe`)

| old | new |
|---|---|
| promptforge-workshop | workshop |
| promptforge-workshop-server | workshop-server |

### mcp-server product

Deleted in step 1 - unused, and git history preserves it whole. If it ever returns, it rejoins as a fourth product named `mcp-server`.

### shared infrastructure

| old | new |
|---|---|
| promptforge-progress | shared-progress |

`shared-progress` is linked by gateway, workshop, and library crates alike (10 dependents) - the `shared-` prefix exists for exactly this class.

### build tooling

| old | new |
|---|---|
| ui-build | build-ui |
| llama-cuda-build | build-llama-cuda |
| make-user-guide | build-user-guide |

The `build-` prefix marks compile-time/CI/doc tooling: `build-ui` is the esbuild-into-OUT_DIR build-script helper (path dependency of two crates, linked into neither), `build-llama-cuda` is the CI-side CUDA packaging binary driven by `llama-cuda-blackwell.yml`, `build-user-guide` assembles the per-crate guides into `guide/promptforge-user-guide.md`. After this rename every crate carries a class marker - the bare-name class is empty.

## What a rename touches (per crate)

For each crate being renamed, ALL of these in the same commit:

1. `git mv crates/<old> crates/<new>` - never delete-and-recreate. Git records no rename flag; history following (`git log --follow`, blame, GitHub's UI) works by content similarity at read time, so rename commits contain moves plus mechanical name edits ONLY: no reformatting, no comment rewriting, no drive-by fixes. A moved file that is also heavily edited falls below the similarity threshold and its history chain breaks.
2. `crates/<new>/Cargo.toml`: `name = "<new>"`, and `[[bin]] name` where present.
3. Root `Cargo.toml` `[workspace.dependencies]`: the `<old> = { path = ... }` key becomes `<new> = { path = "crates/<new>" }`.
4. Every dependent's `Cargo.toml`: the dependency key renames (workspace deps are referenced by key, so `foo = { workspace = true }` becomes the new key).
5. Every `use <old_snake>::` / `<old_snake>::` path in `src/` and `tests/` across the workspace (crate names with hyphens become underscores in Rust).
6. Prose: the crate's own `AGENTS.md`/`README.md`, plus any other crate's docs that name it.
7. `Cargo.lock`: regenerate (`cargo check` rewrites it; commit the result).

## The 9 steps

Each step is one commit, verified green before the next begins. Deletions come first so no rename work lands on a crate that is about to die; renames are sequenced leaf-first within each product so dependents never reference a name that does not exist yet.

### Step 1. Delete promptforge-mcp-server and promptforge-dev

Both are unused; git history preserves them whole. `git rm -r` both crate directories; remove the workspace `Cargo.toml` member/dependency entries; remove both crates from `crates/make-user-guide/src/main.rs`'s list; delete `guide/src/mcp-server.md` and `guide/src/dev-runner.md` and their `SUMMARY.md`/`introduction.md` references; clean `README.md`, `promptforge.md`, and the `prompts.toml` usage comments; regenerate the guide; commit the `Cargo.lock` rewrite. No CI workflow references either crate (verified by grep). The shipped-prompt live semantic resolution assertion in mcp-server's owner tests dies with the product - the commit message records that coverage loss explicitly.
- Verify: `cargo test --workspace --locked` green; `rg "mcp-server|mcp_server|promptforge-dev|promptforge_dev"` returns nothing outside git history, `design/`, `research/`, `CHANGELOG.md`, and `crates/promptforge-core-tests/` (its README names dev; step 2 deletes that crate next).
- Debt risk: none structural - deletion is total and recoverable.

### Step 2. Dissolve promptforge-core-tests into promptforge-core/tests/

The offline fixture suite is live CI coverage of core's public API; the 0.6B scenario harness never runs anywhere. Move the suite, delete the harness. Move `src/suite/` (parsing, shipped, execution) and the `prompts/` fixture tree into `promptforge-core/tests/` as integration tests - the `tests/` boundary preserves the black-box discipline the crate existed for (integration tests see only the public API). Fix the shipped-prompt smoke test's repo-root path for the new `CARGO_MANIFEST_DIR` depth. Core's dev-dependencies gain what the suite needs (`tempfile`, tokio `test-util`; picker and tools are already core dependencies). Delete the scenario harness (`src/main.rs`, `src/scenarios.rs`, `src/gateway/`, the `[[bin]]`, the reqwest/rand/signal dependencies) - recoverable from git if a self-hosted CI leg ever wants it. Remove the crate from the workspace; commit the `Cargo.lock` rewrite.
- Verify: the moved suite runs green under `cargo test -p promptforge-core` with the SAME test count as `cargo test -p promptforge-core-tests` before the move (count both, record in the commit message); `cargo test --workspace --locked` green; `rg "core-tests|core_tests"` returns nothing outside git history, `design/`, `research/`, `CHANGELOG.md`.
- Debt risk: a fixture silently dropped in the move. Mitigation: the before/after test-count comparison, plus the suite's register-by-name discipline (an unregistered fixture is a compile error, not a skip).

### Step 3. gateway product (11 crates)

Rename the 11 gateway crates per the map, leaf-first: `gateway-protocol`, `gateway-config`, `gateway-loopback`, `gateway-routing`, `gateway-local`, `gateway-config-ui`, `gateway-whisper-ffi`, `gateway-stt`, `gateway-transcribe`, `gateway-web-search`, then `gateway` (the binary crate) last. (`promptforge-gateway-build` was deleted in `ef82879f` when build-time native compilation was replaced by prebuilt-artifact downloads - nothing to rename.) The media/search crates carry the `gateway-` prefix like the rest of the family; the library's `promptforge-web-search` keeps its prefix, so no collision. The `gateway` crate's `[[bin]]` becomes `gateway`. Update `crates/gateway-config-ui/ui/package.json` name (`promptforge-gateway-config-ui` -> `gateway-config-ui`) and its `package-lock.json`.
- Verify: `cargo test --workspace --locked` green; `rg "promptforge-gateway|promptforge-stt|promptforge-transcribe|promptforge-web-search-service"` returns nothing outside git history, `design/`, `research/`, and `CHANGELOG.md` (historical documents keep old names - they describe the past); history spot-check: `git log --follow --oneline -- crates/gateway/Cargo.toml` shows the pre-rename history (repeat for one file per renamed crate).
- Debt risk: a missed `use` path in a feature-gated module compiles away locally. Mitigation: CI's `--all-features` runs cover them; the step's own `cargo test --workspace --all-features` run is the local proof.

### Step 4. library, shared, and build tooling (5 crates)

Rename `workshop-agent` -> `promptforge-agent` (the crate exists on disk from the completed harness plan; it joins the library product - cloud/headless agent hosts are anticipated, so the executor rides with the library rather than promoting out of the workshop later; update its AGENTS.md charter sentence to say library product), `promptforge-progress` -> `shared-progress` (10 dependents across all three products; the `shared-` prefix marks crates no single deliverable can claim), `ui-build` -> `build-ui` (path dependency of workshop-server and gateway-config-ui - update both Cargo.tomls and the build.rs/build.mjs references), `llama-cuda-build` -> `build-llama-cuda` (update the cargo invocations in `.github/workflows/llama-cuda-blackwell.yml`; the workflow's own filename stays - it names the pipeline, not the crate), and `make-user-guide` -> `build-user-guide` (no references outside its own files and the lockfile - verified by grep). The 10 library crates keeping the `promptforge-` prefix are untouched.
- Verify: `cargo test --workspace --locked` green; `rg "workshop-agent|workshop_agent|promptforge-progress|promptforge_progress|ui-build|ui_build|llama-cuda-build|llama_cuda_build|make-user-guide|make_user_guide"` returns nothing outside git history, `design/`, `research/`, `CHANGELOG.md`, and the `llama-cuda-blackwell.yml` filename.

### Step 5. workshop product (2 crates)

Rename `promptforge-workshop` -> `workshop` (binary `workshop`) and `promptforge-workshop-server` -> `workshop-server` (binary `workshop-server`). Update `crates/workshop-server/ui/package.json` name (`promptforge-workshop-ui` -> `workshop-ui`) and its lockfile. Update the name-assertion test in `crates/workshop/src/main.rs` (`crate_is_named_promptforge_workshop` asserts `CARGO_PKG_NAME`).
- Verify: `cargo test --workspace --locked` green (includes the renamed assertion); `rg "promptforge-workshop"` returns nothing outside git history, `design/`, `research/`, `CHANGELOG.md`.
- Debt risk: the Tauri shell embeds the server binary path. Mitigation: `crates/workshop` build/discover code references the server binary name - grep `workshop-server` in `crates/workshop/src/` and `crates/workshop/build.rs` (if present) and update; the workspace test run plus a `cargo build -p workshop` prove it.

### Step 6. CI, guide generator, and repo-level docs

Files: `.github/workflows/ci.yml` (~40 crate-name references in `--exclude`/`-p` flags and `working-directory` paths), `.github/workflows/cuda.yml` (3 references), `crates/build-user-guide/src/main.rs` (the hardcoded crate list - renamed in step 4), root `README.md`, root `AGENTS.md`, `promptforge.md`, `workshop.example.toml` comments, `.gitignore` (3 crate paths), `.gitattributes` (5 crate paths). Regenerate the guide: `cargo run -p build-user-guide` rewrites `guide/promptforge-user-guide.md` from the renamed per-crate guide files - note the per-crate guide FILENAMES (`user-guide-promptforge-cli.md` etc.) stay as-is in this plan (they are inputs, not crate names); only the crate-name strings inside `main.rs`'s list change.
- Verify: `cargo test --workspace --locked` green; the regenerated guide diff shows only name changes; `rg "promptforge-gateway|promptforge-stt|promptforge-transcribe|promptforge-web-search-service|promptforge-workshop|promptforge-mcp-server|promptforge-core-tests|promptforge-dev|promptforge-progress"` across the repo returns nothing outside git history, `design/`, `research/`, `CHANGELOG.md`, and the per-crate guide filenames.
- Debt risk: CI exclusion lists silently stop excluding a renamed crate (an old name in `--exclude` is a no-op, not an error). Mitigation: after editing, diff the CI crate list against `cargo metadata` output - every excluded crate must exist under its new name.

### Step 7. Create the promptforge facade crate

New crate `crates/promptforge/` - the library's integrator-facing surface. Two modules, no logic, no new types:

```rust
pub mod pipeline {
    //! Document prompts (.md): sections, prose, the built-in tool loop.
    pub use promptforge_core::execute::run;
    pub use promptforge_core::execute::{RunConfig /* and its error type */};
}
pub mod agent {
    //! Agent programs (.lua): the Lua program owns the loop.
    pub use promptforge_agent::{run_agent as run, AgentConfig, AgentError};
}
```

The facade presents its own vocabulary (`pipeline`, matching the root AGENTS.md "pipeline runtime" phrasing) without touching core's `execute` module name. Dependencies: `promptforge-core` and `promptforge-agent` only - integrators needing substrate types depend on the substrate crates directly (lean edges stay lean). Ships with a canonical-format AGENTS.md (charter: facade only, re-exports with docs, never grows logic) and a README showing the two entry points. One collision to handle: package `promptforge` (this lib) coexists with the `promptforge` binary from `promptforge-cli` (separate namespaces), but `cargo run -p promptforge` stops resolving - grep docs, CI, and scripts for the bare flag (`-p promptforge` followed by a space or end-of-line, not `-p promptforge-core` etc.) and point them at `promptforge-cli`.
- Verify: `cargo test --workspace --locked` green; `cargo doc -p promptforge` renders both modules with the re-exported items documented; a doc-test or integration test calls `promptforge::pipeline::run` and `promptforge::agent::run` signatures (compile-level proof the paths resolve).
- Debt risk: the facade accretes logic over time. Mitigation: the AGENTS.md charter forbids it, and step 8's audit reviews the file like every other.

### Step 8. AGENTS.md audit

Review all 24 repo-owned AGENTS.md files (root + 23, counting the facade's new one and the nested `workshop-server/ui/AGENTS.md`; `third_party/llama.cpp/AGENTS.md` is upstream, excluded), and create the one file the audit notes call for (`gateway-whisper-ffi` - 25 files at the end). Every file is rewritten to one canonical format:

1. **One-sentence paragraph** stating what the crate owns (the charter sentence).
2. **Bullets, sorted descending by blast radius: how many lines of code would be affected if the rule were violated.** A charter bullet that guards an entire crate's dependency direction outranks a bullet that governs one function's error handling, which outranks a style preference. Neighboring bullets with similar blast radius can swap freely - the ordering needs to be defensible, not provable. No headings beyond the title, no sections, no examples - if a rule needs an example to land, the example is inline in the bullet, not a separate block.

The content standard: every bullet earns its keep by preventing technical debt or encoding hard-fought knowledge - a constraint no type or test enforces, an exception that was learned the hard way, an invariant that would be violated if unstated. Bullets that narrate what the code already says, restate the crate name, or describe aspirations rather than constraints are deleted. Bullets referencing renamed crates are updated (steps 3-5 already did this mechanically; this step catches prose that describes relationships, not just names). The root AGENTS.md gets the same treatment plus one addition if missing: a bullet stating that crate names encode product membership per the three-product taxonomy.
- Verify: every file matches the format (one sentence, then sorted bullets, nothing else); every surviving bullet answers "what breaks or drifts if this bullet is deleted?" with a concrete answer; `cargo test --workspace --locked` green (AGENTS.md changes are prose-only, but the suite confirms nothing else moved).
- Debt risk: an over-aggressive cut deletes a constraint that was load-bearing. Mitigation: each deletion is justified in the commit message with why the bullet was redundant or stale; when in doubt, keep.

#### Step 8 audit notes (snapshot 2026-09-01, pre-rename names)

Every note below was written against the files as they existed on 2026-09-01, in pre-rename names (the rename map resolves any name; the missing-files list uses post-rename names). The harness plan completed on 2026-09-02, so the tree is stable - still, re-verify each note against the file at execution time.

**Cross-cutting cuts (apply everywhere):**

- Delete the per-crate bullet "Every public item carries a `///` doc comment; behavior changes ship with tests in the same change." wherever it appears (roughly 15 files). The root AGENTS.md already mandates both workspace-wide, and nested files are always read together with the root. `promptforge-gateway-config` already omits it - proof the per-crate copy is not load-bearing.
- Delete `## Rules` and all other section headings; the canonical format is title, charter sentence, sorted bullets.
- Delete the meta preamble "These rules bind X. The repo-root AGENTS.md applies on top." (workshop, workshop-server, ui). State the layering once in the root instead.
- Root: "Two products ship from this workspace" is stale - the taxonomy is three products (gateway, promptforge library, workshop) after step 1 deletes mcp-server. Fix it and add the taxonomy bullet: crate names encode product membership.
- Root: the Comments section's rust example block becomes an inline clause in its bullet - the format forbids example blocks.
- Duplication: the no-native-compilation rule appears in root, gateway, and gateway-local. Root keeps the general rule; the gateway crates keep only their bundle-specific bullets.

**Per-file notes:**

- **Root**: convert sections to sorted bullets. "Do more with less" is bullet one - highest blast radius in the repo. Keep: plan-is-the-spec, no-files-outside-the-repo, feature-gating rules, the progress rule, the comments policy, the verify commands. Fix the stale two-products line.
- **promptforge-core**: consolidate the four "follows the `tools` precedent" re-export bullets into one precedent bullet; keep the boundary list (which vocabulary lives in which crate) as a second bullet. All content earns its keep.
- **promptforge-core-support**: content compliant. The Observer-report-only / EventLog split bullet is the newest hard-fought line - keep it near the top.
- **promptforge-lua**: keep all bullets; mechanical rename of `workshop-agent` in the doc(hidden)-seam bullet.
- **promptforge-model-client**: keep all bullets ("never a universal client" is the hard-fought one); mechanical rename in the seam bullet.
- **workshop-agent** (`promptforge-agent` by audit time): charter sentence changes to library membership (step 4 covers this); the four bullets all keep.
- **promptforge-parser, promptforge-store, promptforge-tools, promptforge-webfetch, promptforge-web-search, promptforge-gateway-routing, promptforge-gateway-protocol, promptforge-progress**: content fully compliant - format conversion only (plus mechanical renames where other crates are named).
- **promptforge-gateway**: keep the feature bullets (`local` additive, `llama-cuda` bundle, `web-search` additive); the build-crate-split bullet is stale (`promptforge-gateway-build` was deleted in `ef82879f`) - delete it unless the file has already been amended; cut the duplicated no-compile bullet (root covers it); mechanical renames.
- **promptforge-gateway-local**: same duplication cut; keep the boundary bullets; mechanical renames.
- **promptforge-gateway-config**: keep all four bullets (validation-before-exit is the top one).
- **promptforge-workshop**: convert the three sections to bullets; write a real charter sentence (desktop shell: Tauri window, same-origin policy, platform bridges, lifecycle) replacing the meta preamble. The unsafe-bridge bullet is top (dense COM, the crate's only unsafe); event-loop-never-panics is second.
- **promptforge-workshop-server**: the largest file (5.6 KB), and line-by-line review says ~70-75% is load-bearing - this is a compression job, not a prune. Convert the eleven sections to ~13-15 sorted bullets: two-zone error policy and embedding hygiene (never panic/exit) at the top; transcription boundary and the WebSocket session model (with the agent-sessions carve-out) next; then delivery contract, drop guards, feature gating, router/module ceilings, tests, asset serving. Specific cuts: the meta preamble; the embedding-hygiene overlap with the root (keep only the crate deltas: no unconditional tracing init, no argument-ignoring `OnceLock`, loopback-only binding); and the "target-state: refactor-era" line if the refactor is done by execution time. Re-read at execution time - the harness run is actively amending this file.
- **ui**: convert to bullets; the layer-import rule (defined once, enforced three ways) is the top bullet. The vendored-code bullet may be moot if the harness plan's step 16 deletes `ui/src/chat/` - check at execution.
- **promptforge-web-search-service**: content compliant; mechanical renames (`gateway-config`).

**Missing files:** `promptforge-cli`, `promptforge-gateway-config-ui`, `promptforge-gateway-loopback`, `build-user-guide`, `build-ui`, and `build-llama-cuda` have no AGENTS.md (`promptforge-mcp-server`, `promptforge-dev`, and `promptforge-core-tests` also have none, and steps 1-2 delete them; `promptforge-gateway-build` was deleted in `ef82879f`). `whisper-ffi` (renamed `gateway-whisper-ffi` in step 3) has none and SHOULD gain one - its FFI invariants (unsafe confined, every unsafe block carries a SAFETY comment, Drop-on-raw-pointers) are exactly the constraints code cannot enforce. Otherwise create only where a real constraint exists that code cannot enforce (candidate: loopback's scope); do not create ceremonial files - absence is cheaper than ceremony.

### Step 9. Final sweep and guide regeneration

Repo-wide audit commit: re-run the full grep matrix from steps 1-5, fix any stragglers, regenerate the guide one final time, and confirm `cargo test --workspace --locked` and `cargo doc --workspace --no-deps` are green. This commit exists to catch whatever the per-step greps missed - if it is empty, it does not happen (no empty commits).
- Verify: the full grep matrix clean; full suite green.
- Debt risk: none - this is the net.

## Explicitly NOT renamed (recorded so nobody "fixes" them later)

- The 10 library crates keeping `promptforge-` - the prefix is the product name there.
- Per-crate guide filenames (`user-guide-promptforge-*.md`) - they are build-user-guide inputs; renaming them is cosmetic churn with no taxonomy benefit.
- The Tauri identifier `com.promptforge.workshop` and `productName: "PromptForge"` - user-facing app identity, not crate taxonomy.
- The `~/.promptforge/` user config directory - user-facing, and renaming it orphans existing installs.
- Historical documents: `design/`, `research/`, `CHANGELOG.md`, git history - they describe the past under the names that were true then.
- The GitHub repository name (`cppalliance/promptforge`) and the `repository` field in workspace Cargo.toml.

## Data-flow check (performed)

- Steps 1-2 (deletions) run first so no rename work lands on a dying crate. Steps 3-5 (renames) are independent of each other (different crate sets) but each is internally ordered leaf-first; running them in sequence keeps every intermediate commit green. Step 6 needs all of 3-5 (it references final names). Step 7 (facade) needs 4 (`promptforge-agent` must exist under its final name). Step 8 needs 3-7 (it audits prose that must describe final names, including the facade's new AGENTS.md). Step 9 needs 8.
- No step consumes an artifact before it exists: directory moves and name changes are atomic within each commit.
- The one cross-product dependency: `workshop` (step 5) depends on `gateway` (step 3) via the bundled-gateway feature, and `cli`/`dev` depend on library crates that do not rename - so no step is blocked by a later step's rename.

## What this does NOT change

- Crate contents, module structure, public APIs, behavior - renames and deletions only. The one additive exception is step 7's facade crate, which is pure re-exports.
- Core's `execute::run` path and module name stay exactly as they are; the facade presents `pipeline` as its own vocabulary via re-export.
- The flat `crates/*` layout (product subdirectory grouping was considered and declined - the name prefix already encodes the product, and the flat list self-sorts by prefix).
- The agentic-harness plan, which must be fully complete before this plan starts (hard precondition, stated above).


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: crate taxonomy rename

## Why the plan exists

The rename grew out of the agentic-harness work, where crate names stopped matching what crates actually were. The immediate trigger was `promptforge-gateway-client`, which the gateway executable did not use. The user: "promptforge-gateway-client is a confusing name. I want 'promptforge-gateway' to be exclusively to mean 'every crate needed to build promptforge-gateway.exe and nothing more and nothing less'". That crate became `promptforge-model-client`, and the naming principle generalized into the whole taxonomy: "the goal here is to make it crystal clear. You can immediately tell which crate belongs to which deliverable."

The product division was stated by the user verbatim: "There are four products: gateway: the server app that proxies inference / promptforge: the library that gives you execute::run / workshop: the windowed desktop application (which also bundles the gateway) / mcp-server: a server app that lets you run promptforge prompts via mcp". The plan later cut mcp-server to three products.

## Why the deletions (step 1-2)

The user's framing, verbatim: "I feel like core tests and maybe even MCP server... they're liabilities because I don't use them anymore... it's just extra crap that we're just dragging around. And we can always recreate them later... using AI could just shit out a complete implementation again. So why are we dragging these along?" Deletion was cheap because git history preserves everything and regeneration is considered nearly free. Core-tests was dissolved rather than deleted because the offline fixture suite still had value: "The offline fixture suite. Okay, but why don't we just move those into the main?" (paraphrase of intent: keep the fixtures, drop the crate and the scenario harness).

## Naming decisions and discarded alternatives

- Product subdirectories under `crates/` were the user's first idea ("in Rust do all the crates have to be immediately below crates/ or can a repo give it more structure?"). Declined: unique flat names plus prefixes already encode membership, and the flat list self-sorts.
- Ultra-short prefixes were evaluated and rejected: "evaluate this change: rename promptforge to pf, rename workshop to ws, rename gateway to gw".
- `promptforge-lib` was floated for the library facade ("integrators who want the lib maye we call it promptforge-lib?") and rejected; the facade carries the bare brand name `promptforge`.
- crates.io was abandoned as a distribution channel, which freed naming from registry concerns: "someone is already squatting on promptforge in crates.io so I have decided to never publish there, and make signed installers the primary distribution mechanism."
- The `promptforge-` prefix survives only on the library because the prefix IS the product there. Elsewhere it is dropped: "literally 'workshop-agent' - we are moving away from promptforge- prefix".
- `gateway-web-search` kept its full name against a shorter suggestion: "gateway-web-search please. we are note dropping the prefix" (sic).
- `shared-` and `build-` prefixes were user directives: "lets rename it to shared-progress"; "rename anything ending in -build, to starting with build- instead"; "rename make-user-guide to build-user-guide".
- The facade API shape was dictated verbatim: "I want: promptforge::pipeline::run / promptforge::agent::run".
- Agent crate placement was actively debated. The user first asked "why should agent be a workshop product? I feel like agent should ride right along the core", then settled it: "my point is that having an agent in the cloud means putting the agent in workshop is the wrong call". Hence `workshop-agent` -> `promptforge-agent`, joining the library.
- Folding mcp-server into the gateway process was considered ("should the gateway also offer the mcp-server in the same process?") but mooted by deleting mcp-server as unused.
- History preservation was a hard requirement: "I want git renames so we keep history" - this is why the plan mandates `git mv` and move-plus-mechanical-edits-only commits.

## Step 8 (AGENTS.md audit) design intent

The audit was the user's idea, with the standard stated verbatim: "I want the plan to include a review of every AGENTS.md file and make sure each line is earning its keep. Charter lines are the highest value. Generally speaking, AGENTS.md should have only things that prevent technical debt, and encode hard-fought knowledge/exceptions." And the format: "every AGENTS.md should have the same format: one sentence paragraph then bullets, and the bullets are sorted descending by strength at preventing vibe-induced technical debt". On sorting strictness: "the measure is how many lines of code would be affected" with "a little fuzz at the margins is probably harmless". Compressing the largest file with ASD-STE100 was asked about ("should we use ASD-STD100 to compress?") and not adopted; the plan treats workshop-server as a compression job by hand instead.

## Sequencing constraint

The plan must run only after the agentic-harness plan finishes. This was burned in after the assistant edited the in-flight harness plan by mistake: "what the fuck? no! you just updated a plan that is already being executed... can you roll back that edit?" followed by "to be clear, this rename plan... should be written with the understanding it will run AFTER the other plan is finished executing".

## Run-time deviations (from the run chats)

- Squashing was considered and rejected. User: "question: should we squash the whole changeset down to 1 commit?" then "so its best left alone?" - the nine green commits were kept as-is.
- `promptforge-cli` was deleted during the run even though the plan kept it unchanged. User: "I say we delete promptforge-cli completely and do away with the problem entirely" and "the cli is useless, and we dont use crates.io anymore I gave up on that. and we should delete promptforge.md". Both were removed as extra work beyond the plan's nine steps.
- After the push, CI on the PR failed on Format and then Clippy; follow-up fixes were required beyond the plan's local verification gates. Local `cargo test --workspace --locked` did not cover what CI's format and clippy jobs caught.
