---
name: prompt reads debt removal
overview: Remove the one cheap fix the prompt-namespace-reads work left, adopt two rejected candidates, and fix every pre-existing finding the debt collector exposed, including marking closed plans' todos completed going forward and across vibe/ history.
todos:
  - id: parser-cleanup
    content: "D1-5, C-1: split build_sections into four named checks; drop synthetic_section's items parameter and update its four call sites"
    status: pending
  - id: scope-module
    content: "D1-14, D1-15: add h1_visible_sections beside visible_sections and use it at the three H1 sites; rename resolve_sibling to resolve_visible with its tests and doc links"
    status: pending
  - id: names-docs
    content: "C-2, C-3, X-1: rename the fanout-arm test, fix the reserved tool-alias docs in globals.rs and parse.rs, set the two spec status lines to Active. (promptforge-design commit)"
    status: pending
  - id: plan-history
    content: "D1-16: vibe coder close step marks todos completed (tools-public commit); flip todo statuses in every closed vibe/ plan, preserving line endings"
    status: pending
  - id: exit-gate
    content: "Full exit gate: both nextest runs, both clippy runs, fmt, docs builds, xtask api --check, build-xtask tests, docs-claims, and the rg checks"
    status: pending
isProject: false
---

# prompt reads debt removal

<product-contract>

## Product Requirements

- Scope and target work
  - Debt analysis target: the `vibe/2026-10-10-2-prompt-namespace-reads.md` plan, baseline `abe218a36`, endpoint and disposition ref `460da3f9e`, target commits `475fc39cb`, `4dc155672`, `7a80775b7`, `460da3f9e` in `c:\Users\Vinnie\cursor\promptforge`. Its spec commit `6341ac8` in `promptforge-design` was outside the analyzed repository.
  - The user expanded the scope: every unrelated pre-existing finding is fixed here, and the rejected candidates were evaluated for adoption (two adopted).
- Cleanup goals
  - Remove the orphaned `items` parameter of the parser's `synthetic_section` test helper (C-1).
  - Rename a test whose name is now half false (C-2), and correct the two new spec status lines (X-1).
  - Fix the pre-existing findings: split `build_sections` into named checks (D1-5), give H1's visible set one helper (D1-14), rename `resolve_sibling` to `resolve_visible` (D1-15), correct two stale doc comments about reserved tool aliases (C-3), and mark closed plans' todos completed, going forward in the vibe coder and across `vibe/` history (D1-16).
- Non-goals
  - Any change to author-visible behavior: `prompt.section`, `prompt.list`, `call`, `jump`, `fanout`, error messages, and the reserved-name list stay as they are.
  - Task execution behavior, the Harness, the public facade surface (`crates/promptforge/public-api.txt` stays the same).
  - The rejected candidates listed under Deferred.
- Success criteria
  - Every work item's check passes, and the full exit gate in the Testing Plan passes.
  - `rg -n resolve_sibling crates` prints nothing, and H1's visible set is computed in exactly one function.
  - No tracked `vibe/` plan except the active one has a frontmatter todo reading `pending` or `in_progress`, and the vibe coder's close step marks todos completed.

## Functional Specification

### Debt Inventory

- Debt added
  - None introduced or worsened. The analysis and challenge found no incorrect behavior, contract contradiction, or prospective debt with a present reversal burden in the target.
- Cheap fixes
  - C-1, from `7a80775b7`: deleting `exec_flow.rs::list_from_section_ambiguous_error_is_loud` removed the only callers that passed non-empty `items` to `crates/promptforge-internal/parser/src/test_support.rs::synthetic_section`. All four remaining call sites (the `sibling` helper in `engine/src/heading_address-tests.rs`, three calls in `engine/src/execute/tests/exec_flow.rs::duplicate_top_level_section_names_error_loudly`) pass `Vec::new()`. Dead code the target orphaned; no reservation. Target state: the parameter is gone.
- Adopted rejected candidates
  - C-2, residual-but-acceptable: `engine/src/execute/tests/suite/exec_flow/fanout_arms.rs::call_and_list_inside_a_fanout_arm_reject_non_visible_sections` now asserts `prompt.list('## Parent') == nil`, so "reject" is false for its list half. Adopted because the rename is free and the name misleads.
  - X-1, the analysis's out-of-scope observation: `promptforge-design/design/architect-promptforge-prompting-language.md` lines 364 and 370 read "Active: new in this plan." and "Active: the list-section read, now `nil` on a miss." In that spec "this plan" means the spec's own plan, so the first wrongly claims `prompt.section` was part of it, and "now" is change-relative. The spec already uses a bare `Active.` elsewhere. Target state: both lines read `**Implementation status:** Active.`
- Exposed pre-existing debt, fixed by user scope expansion
  - D1-5: `crates/promptforge-internal/parser/src/build.rs::build_sections` runs from line 205 to `Ok(result)` at line 296, about 92 lines, holding the orphan-level check, the empty-name check, list detection, and the duplicate-sibling check inline. Target state: each check is a named helper and `build_sections` reads as the sequence of steps.
  - D1-14: H1's visible set (the whole top-level slice) is spelled out at three sites with the same comment: `engine/src/execute/section_context-construct.rs` line 171 (`new_live_h1`), `engine/src/execute/scheduler/walk.rs` line 290 (`resolve_chain_target`'s H1 branch), and `engine/src/execute/scheduler/h1.rs` line 148 (`end_live_h1_at_jump`). Target state: one helper beside `visible_sections`, so one module computes every visible set.
  - D1-15: `engine/src/heading_address.rs::resolve_sibling` resolves over siblings and direct children. Target state: renamed `resolve_visible`, pairing with `find_visible`.
  - C-3: `lua/src/globals.rs` module doc (lines 29 to 33) says "A frontmatter tool alias installs as a global of its own name, so the parser refuses one that is reserved", and `parser/src/parse.rs` line 32 lists "a tool alias is a reserved name" under `Frontmatter` errors. The frontmatter has only `plugins`, `args`, and `models`, the parser never calls `reserved_name`, and only `lua/src/prelude.rs` checks reserved names (for Plugin prelude globals). Target state: both docs state what is true.
  - D1-16: 133 tracked plan files under `vibe/` carry frontmatter todos reading `status: pending` (5 of them also `status: in_progress`), though every one is closed. The vibe coder (`c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md`, Run Sequence step 5) marks progress only in step headings and never touches todos. Target state: the close step marks the plan copy's todos completed, and every closed plan's todos read `completed`.
- Rejected candidate counts (22 total; 2 adopted above)
  - residual-but-acceptable 7 (D1-1, D1-2, D1-4, D1-7, D1-8, D1-9, C-2): intended contract or bounded cost with no demonstrated consequence; C-2 adopted anyway.
  - weak/speculative 3 (D1-3, C-4, C-6): no consequence shown.
  - false 6 (D1-6, D1-10, D1-11, D1-12, D1-13, C-5): the code contradicts the candidate.
  - unrelated pre-existing 5 (D1-5, D1-14, D1-15, D1-16, C-3): all fixed here by scope expansion.

</product-contract>
<implementation-contract>

## Technical Design

- Paths are relative to `crates/promptforge-internal/` unless they start with `crates/`, `vibe/`, `../`, or name a sibling repository.
- Parser (D1-5, C-1):
  - `parser/src/build.rs`: `build_sections` keeps its signature and behavior. Its inline checks move to private helpers with the exact error kinds, spans, and message texts they produce today:
    - `fn check_child_level(heading: &Heading, parent_level: u8) -> Result<()>`: the orphan-heading error
    - `fn section_name(heading: &Heading, level: u8) -> Result<String>`: the empty-heading error, returning the name
    - `fn list_items(blocks: &[Block], name: &str) -> Result<Vec<String>>`: the list-only detection and `parse_bullet_items` call
    - `fn check_unique_sibling(siblings: &[(String, u32)], name: &str, heading: &Heading, heading_abs_line: u32) -> Result<()>`: the duplicate-sibling error naming both lines
  - `parser/src/test_support.rs`: `synthetic_section(name: &str, level: u8, blocks: Vec<Block>) -> Section` sets `items: Vec::new()`; its doc drops "a list of items".
- Visible sets and lookup (D1-14, D1-15):
  - `engine/src/execute/walk_target.rs` gains, beside `visible_sections`:

    ```rust
    /// The H1 pass's visible set: the whole top-level slice. H1 is
    /// section 0, so it excludes nothing and has no children.
    pub(super) fn h1_visible_sections(prompt: &Prompt) -> &[Section] {
        promptforge_parser::detail::sections(prompt)
    }
    ```

    The three H1 sites call it (`new_live_h1` keeps its `.to_vec()`, since the closure owns its set), and their duplicated "section 0 excludes nothing" comments go. Other `detail::sections` uses (`scheduler.rs` `SlicePath::resolve`, `context.rs` section count, the emptiness checks in `scheduler/drive.rs` and `scheduler/h1.rs` line 101) are not visible sets and stay.
  - `engine/src/heading_address.rs`: `resolve_sibling` becomes `resolve_visible` with the same signature, body, and messages. Every caller, doc link, and test follows: `walk_target.rs` (2 mentions), `scheduler/walk.rs` (2), `scheduler/h1.rs` (1), `heading_address-tests.rs` (15, including the seven `resolve_sibling_*` test names, which become `resolve_visible_*`). That test file's module doc, "Tests for resolving a fanout worker heading among sibling sections.", becomes "Tests for heading lookup over a visible set."
- Docs (C-3, X-1):
  - `lua/src/globals.rs` module doc: the sentence about frontmatter tool aliases becomes "A Plugin prelude global may not take one."
  - `parser/src/parse.rs` `# Errors`: drop "a tool alias is a reserved name," from the `Frontmatter` list.
  - `../promptforge-design/design/architect-promptforge-prompting-language.md` lines 364 and 370: `**Implementation status:** Active.`
- Test name (C-2): `call_and_list_inside_a_fanout_arm_reject_non_visible_sections` becomes `call_raises_and_list_reads_nil_for_non_visible_sections_in_a_fanout_arm`; body and doc comment unchanged.
- Plan history (D1-16):
  - `c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md`, Run Sequence step 5 becomes: "Set every frontmatter todo `status:` in <PLAN PATH> to `completed`, then commit that change with the deletion of `vibe/ACTIVE` with subject `Close plan: <words>`, a blank line, and the plan's `Plan:` trailer." Append one attribution line at the file's end in the existing form: `*2026-10-10 - Claude Opus 5.5 (Cursor agent). Close step marks the plan copy's todos completed.*`
  - `vibe/`: in every tracked plan file except the one `vibe/ACTIVE` names, within the YAML frontmatter only (line 1 `---` through the next line that is exactly `---`), lines matching `^(\s+)status: (pending|in_progress)\s*$` become `$1status: completed`. Each file keeps its line endings and its final newline, and no other byte changes. Git stores every tracked file with LF; in the worktree `vibe/2026-10-09-3-models-loop-in-rust.md` is CRLF, and `vibe/2026-09/2026-09-26-4-store-into-vfs.md` holds two lone CR bytes that make git classify it as binary.
- Cross-repository commits: edits to `promptforge-design` and `tools-public` are committed in those repositories as their own commits, staging only the named file (`promptforge-design` holds an unrelated untracked `research/harness-survey-subagent-task-tools.md`). Sub-agents leave them unstaged; the session running the step commits them after the coding sub-agent returns and before that step's `promptforge` commit, and amends them after any later round that changes them. Nothing is pushed.
- No public interface, persisted or wire format, component ownership, dependency direction, or trust boundary changes.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused
  - D1-5: `cargo nextest run --locked -p promptforge-parser --all-features` passes with no test edits; the orphan, empty-heading, duplicate-sibling, list, and line-mapping tests (`parser/src/tests/headings.rs`, `tests/lists.rs`, `tests.rs`, `build-tests.rs`) pin every message, kind, and span.
  - C-1: the build is the check; every caller compiles against the three-argument `synthetic_section`.
  - D1-15: the seven `resolve_visible_*` tests and `find_visible_returns_none_for_an_address_not_in_the_set` pass; `rg -n resolve_sibling crates` prints nothing.
  - D1-14: the H1 suites pass unchanged (`suite/exec_flow/h1_control.rs`, `suite/exec_flow/prompt_reads.rs::prompt_reads_work_in_h1_and_the_shared_library`, `tests/scheduler/live_h1/`); `rg -n "detail::sections\(" crates/promptforge-internal/engine/src --glob "!**/tests/**" --glob "!*-tests.rs"` lists only `h1_visible_sections`, `scheduler.rs`, `context.rs`, `scheduler/drive.rs`, and `scheduler/h1.rs` line 101.
  - C-2: `cargo nextest run --locked -p promptforge-engine --all-features call_raises_and_list_reads_nil` runs one test and passes.
- Docs
  - C-3: `rg -n "tool alias is a reserved|parser refuses one that is reserved" crates` prints nothing; the docs build passes.
  - X-1: `rg -n "Active: new in this plan|now .nil. on a miss" ../promptforge-design/design/architect-promptforge-prompting-language.md` prints nothing, and lines 364 and 370 read `**Implementation status:** Active.`
- Plan history (D1-16)
  - `git grep -n -E "^\s+status: (pending|in_progress)\s*$" -- vibe` lists only the active plan's lines.
  - `git diff --text --numstat -- vibe` shows equal added and deleted counts per file, and every changed line in `git diff --text -U0 -- vibe`, apart from the `---` and `+++` file headers, matches `^[-+]\s+status: `. Without `--text`, git shows the binary-classified plan file as `-` and hides its changes.
  - `git ls-files --eol vibe` still shows every tracked file `i/lf` except `vibe/2026-09/2026-09-26-4-store-into-vfs.md` (`i/-text`), with `w/crlf` only on `vibe/.cursorignore` and `vibe/2026-10-09-3-models-loop-in-rust.md`.
  - `rg -n -F "Set every frontmatter todo" c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md` finds exactly one line, in Run Sequence step 5.
- Exit gate
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - both clippy runs with `CARGO_BUILD_WARNINGS=deny`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps`, both with `RUSTDOCFLAGS="-D warnings"`
  - `cargo +nightly-2026-09-05 xtask api --check` (or the nightly `crates/build-xtask/src/api/toolchain.rs` names), `cargo test -p build-xtask`, and `node --test test/docs-claims.mjs` from `crates/workshop/ui`

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - `resolve_visible` is the new name: it pairs with `find_visible` (`Option` versus not-found error) and says what it searches. Consequence: seven test names and five call or doc sites change in one commit.
  - H1's helper returns a borrowed slice, so `resolve_chain_target` and `end_live_h1_at_jump` keep resolving without a clone; only `new_live_h1` clones, as today.
  - `build_sections`'s helpers stay private to `build.rs` and keep today's messages, so no test changes and the 500-line ceiling is not approached.
  - C-1 removes the parameter rather than adding a list-section constructor; a future test that needs one adds it then, at the same cost.
  - The history flip is a one-off data edit by the session, not committed tooling, so no ratchet or parser of plan files enters the repository.
- User-resolved choices
  - Scope expansion: "for any pre-existing finding, I want it fixed".
  - D1-16: fix it going forward in the vibe coder and also flip every closed plan in `vibe/` (chosen over forward-only and leaving it).
- Rejected alternatives, assumptions, and risks
  - Not adopted: D1-1 and D1-2 (the chosen design), D1-3 (one parse function, two callers, no divergence), D1-4 (the compiler's bound check covers every site), D1-6 (forced by `parser -> lua`), D1-7 (a second callback was rejected in the target plan), D1-8 (pre-existing deep-clone pattern; `Arc`-shared sections is a wider refactor with no measured cost), D1-9 (unreachable branch still covered by `resolve_visible_rejects_more_than_one_match` and `duplicate_top_level_section_names_error_loudly`), D1-10 (same as `models` and `messages`; preludes cannot see it), C-4 (no non-test `Prompt` comparison), C-6 (a parameter placeholder in error text; changing it churns pinned messages).
  - Risk: flipping history marks every todo `completed`, including any a past run skipped; the step headings and git log stay the authoritative record.
  - Risk: the session running this plan may hold the vibe coder's old close step. This plan's own close commit therefore sets this plan copy's todos to `completed` explicitly, whatever version the session read.
  - Assumption: `vibe/ACTIVE` names this plan's copy during the run, so the history flip excludes it.

### Deferred and Out of Scope

- D1-8 visible-set clone cost: revisit if large prompts show memory pressure, with `Arc`-shared sections or `Arc<str>` Markdown.
- D1-3 typed heading address: revisit if a second heading parser or a parse divergence appears.
- C-6 `models.infer takes (prompt)` wording: revisit if authors report confusing it with the `prompt` namespace.
- C-4 `Prompt` equality including raw Markdown: revisit if a non-test consumer starts comparing prompts.
- The spec commit `6341ac8`'s other content was not analyzed beyond X-1: revisit in a debt run that targets `promptforge-design`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway`. `crates/gateway/app` is the only `default-members` entry, so plain `cargo build` builds just the gateway. Build scripts bundle the UIs into `OUT_DIR`, so a fresh clone first runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`, as every `.github/workflows/ci.yml` cargo job does. The desktop app is an explicit `cargo build --locked -p workshop`, and CI first builds `cargo build --locked -p gateway --no-default-features` and stages it with `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source target/debug/promptforge-gateway[.exe]` (removed afterwards with `node tools/stage-gateway-sidecar.mjs remove --target <triple>`). Windows links with `rust-lld` and a static CRT (`.cargo/config.toml`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`, dropping `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file runs with `node --test <file>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, without `--all-features` for the three workshop crates above. CI adds `cargo nextest run --locked -p workshop-workspace --all-features` (Windows path jail) and `cargo nextest run --locked -p workshop-server --features headless`. JS packages: `npm test --workspace <ui|look|platform>` from `crates/workshop`, or `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `build-xtask`, whose tests are the boundary and structural checks (`cargo xtask tidy` prints the same report). Also run `node --test crates/workshop/ui/test/docs-claims.mjs` (wording scan, no `npm ci` needed), `npm test --workspaces --if-present` in `crates/workshop`, and `npm test` in `crates/gateway/config-ui/ui`. CI's Windows and Linux workshop jobs also run five named Gateway process-ownership race tests with plain `cargo test` (`gateway-api-discovery`, and `gateway --no-default-features --features test-fixtures --test it`). Nextest config is `.config/nextest.toml`: 60 s slow timeout terminated after 3 periods, 250 ms leak timeout, STT crates in a capped `heavy` group, and longer timeouts for the realtime STT tests. `tools/*.test.mjs` exist but no CI job runs them.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`). Also `cargo check -p gateway --no-default-features` for the headless build shape (the only allowed standalone `cargo check`), `npm run typecheck --workspaces --if-present` in `crates/workshop`, `npm run typecheck` in `crates/gateway/config-ui/ui`, and `cargo deny check`, `cargo audit`, and `cargo hakari verify` for supply chain. CI also fails if `ring` enters the gateway's normal dependency closure (`cargo tree --locked -p gateway -e normal -i ring --depth 0`) or if a build dirties the tree. `.githooks/pre-push` runs the wording scan, the headless check, the first clippy run, and `cargo deny --log-level error check` when installed.
- Formatter check command: `cargo fmt --all --check` (stable toolchain per `rust-toolchain.toml`, `style_edition = "2024"` in `rustfmt.toml`; `.githooks/pre-commit` runs it).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`. Under the same flags CI also runs `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps` with default features, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and on Windows `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` against the committed `crates/promptforge/public-api.txt` (the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`), plus `cargo nextest run --locked -p build-xtask --run-ignored only` on that nightly for the rustdoc JSON fixtures. The docs site is `cargo xtask site [--books-only]`, which needs `PROMPTFORGE_DOCS` pointing at a `promptforge-docs` checkout and `mdbook`, and writes `target/site/`.
- Test placement and naming conventions: Unit tests live in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (174 files use it, for example `crates/promptforge-internal/lua/src/handles.rs` beside `handles-tests.rs`), or in a `tests/` subdirectory of the module once it has three or more files (for example `crates/promptforge-internal/engine/src/execute/tests/`). Integration tests compile as one binary per crate from `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`promptforge`, `harness`), with shared helpers in `support.rs` (or `tests/common/`) and one file per topic (`cancel.rs`, `replay.rs`, `vfs.rs`); data sits in `tests/fixtures/` or `tests/prompts/`. Test function names are snake_case sentences describing the behavior (`a_process_lifetime_lease_recovers_after_its_owner_is_terminated`). Plugin crates enable `promptforge-plugin`'s `test-support` feature from `[dev-dependencies]` only, and `build-xtask`'s `test_support_leak` guard fails any non-dev table that enables an Engine or Harness crate's `test-support`. Doctests are banned in every workspace crate (`no_doctests`). `build-xtask` fixtures that need rustdoc JSON are `#[ignore]`d on stable. JS tests are `node:test` files in `test/**/*.mjs` (workshop `ui`, `look`, `platform`) and `src/**/*.test.mjs` (workshop `ui` and the gateway config UI).
- Directory map: `crates/` holds every crate (52 manifests). `promptforge` is the Engine facade (with `public-api.txt`), and the manifestless `promptforge-internal/` container holds the private Engine crates `types`, `vfs`, `parser`, `lua`, `model-client`, and `engine`. `promptforge-plugin` is the Plugin contract, implemented by `plugin-web`, `plugin-mcp`, and `plugin-user-input`. `harness` is the Harness facade, `harness-internal/runner` its private runner, and `harness-gateway-client` the gateway client. `gateway/` holds the gateway family (`app` binary with `admin`, `auth`, `boot`, `commands`, `dialect`, `error`, `runner`, `tray` modules; `config`, `config-ui`, `local`, `routing`, `protocol`, `cloud-providers`, `web-search`, `logging`, `progress`, and the `stt/` speech subsystem with `api`, `engine`, `backend-whisper`, `whisper-ffi`); `gateway-api-types` and `gateway-api-discovery` are its public pair. `workshop/` holds the Workshop Host: the `desktop` Tauri app, `server`, `server-api`, the tiered library crates, and the `ui`, `look`, `platform` npm workspaces. Also in `crates/`: `shared-error-source`, `shared-loopback`, and `shared-ui` (a TypeScript and CSS package, not a crate); `build-*` tooling (`build-xtask` for structural checks and `cargo xtask tidy|api|site|new-crate`, `build-workshop` behind `cargo workshop`, `build-ceiling` for the file-size limit, `build-ui`, `build-user-guide`, `build-llama-cuda`); and `workspace-hack` (cargo-hakari, configured in `.config/hakari.toml`). Top level: `guide/` (`books/{language,gateway,workshop}/book.toml` only, since chapters come from the separate `promptforge-docs` checkout; `chrome/` and `landing/` site assets), `prompts/` (sample prompts), `tools/` (Node scripts for gateway sidecar staging and live TTS, with tests), `.github/workflows/` (CI, release, nightly, site, STT Miri, and others), `.githooks/`, `.cursor/rules/` (`workshop-architecture.mdc`, `workshop-spa.mdc`), `vibe/` (past plans, tracked), `local/` (gitignored machine-local config and fixtures), and `images/`.
- Component boundaries: `cargo test -p build-xtask` enforces these (`crates/build-xtask/src/product.rs`, `tidy.rs`, and the guard modules it runs). Engine crates (`promptforge*`) depend on no gateway, workshop, Harness, or Plugin crate, and their manifests may not name `tokio`, `tokio-util`, `async-trait`, or `reqwest` outside `[dev-dependencies]`, except an optional entry only `test-support` enables (`engine_deps.rs`). Inside the Engine, `types` and `vfs` are leaves, `model-client` depends on `types`, `lua` on `types`, `vfs`, and `model-client`, `parser` on `lua` and `types`, `engine` on all five, and the `promptforge` facade on all six. `promptforge-plugin` depends on `types` and `vfs`. Plugin crates depend only on `promptforge-plugin`, `shared-*`, `workspace-hack`, and outside libraries. Harness crates name only `promptforge`, `promptforge-plugin`, and `workspace-hack` outside their family (`harness` uses `harness-runner`; `harness-gateway-client` may also name `plugin-web` to implement its `SearchProvider`), and no Harness crate declares `tokio` or `tokio-util` as a normal dependency (`harness_bans.rs`). Gateway crates depend on no Engine, Harness, or workshop crate. Workshop crates reach the gateway only through `gateway-api-types` and `gateway-api-discovery`, and the Engine and Harness only through `promptforge`, `promptforge-plugin`, `harness`, and `harness-gateway-client`. `shared-*` crates depend on no product crate. Containers are private: only `promptforge` and `promptforge-plugin` may enter `promptforge-internal/`, only `harness` may enter `harness-internal/`, and only `gateway-stt` is public within `gateway/stt/`. The `promptforge` and `harness` facades hold only doc-commented `pub mod` groups and single-item `pub use <internal_crate>::path::Item;` re-exports (`facade_shape.rs`), and no file in either facade or its container carries `doc(hidden)` (`doc_hidden.rs`). A retired-symbol list may not reappear in live Engine source (`retired_symbols.rs`). Only `gateway/app`, `gateway/stt/whisper-ffi`, `gateway-api-discovery`, and `workshop/desktop` may relax `unsafe_code` (`unsafe_allowlist.rs`). Only the gateway app's `admin/walled/` modules plus `lib.rs`, `registry.rs`, and `test_support.rs` may name `crate::admin::walled::`. Workshop tiers run vocabulary (`protocol`, `registry`, `support`), then services (`gateway`, `menu`, `status`), then features (`agents`, `run-log`, `user-state`, `workspace`), then `workshop-server`, each depending only downward. The `workshop` desktop app depends on no workshop crate except `workshop-server-api`, which depends on none except `workshop-server`.
- Conventions summary: The four defined terms in `AGENTS.md` (Engine, Harness, Host, Plugin) are capitalized with one meaning each, and Engine crates say "the caller" and never mention the Host (`crates/workshop/ui/test/docs-claims.mjs` scans for this). All 52 crate manifests set `publish = false` and `[lints] workspace = true`, inherit workspace package fields, and take dependencies as `<dep>.workspace = true`; every crate except `promptforge-vfs` and `build-ceiling` depends on `workspace-hack`. Every crate's build script except `workspace-hack`'s runs `build_ceiling::check()`, which fails any file over 500 lines. Manifests comment why each dependency, pin, or feature exists. Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny outside tests, `allow_attributes` is denied so suppressions use `#[expect(..., reason = "...")]`, `unsafe_code` is denied outside the allowlisted crates, `missing_docs` and `unreachable_pub` warn (fatal under the gate), and `clippy.toml` bans process-global installers outside binary entry points. Every `workshop-*`, `harness-*`, and `plugin-*` crate (not the `workshop` app or `harness` facade) carries a `//! ## Invariants` section in its crate docs. Source directories stay flat until a group reaches three files, using kebab siblings with `#[path]` below that. Comments explain only non-obvious constraints and cite upstream issue URLs for workarounds. Error messages are written for model consumption, naming required versus actual. JSON that reaches a recorder round-trips exactly, with sorted keys and `float_roundtrip`. Behavior changes ship with tests in the same change. Cargo features gate only real build constraints. Changes to the facade surface update `public-api.txt`. Commits are one-line imperative summaries (`Replace list_from_section with prompt.section/list`), plans are kept in `vibe/` as `YYYY-MM-DD-N-<slug>.md`, and a finished plan closes with a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Split build_sections into named checks and drop synthetic_section's items parameter [completed]

- Component: Parser cleanup
- Placement: first. Engine scope module depends on it: both edit `engine/src/heading_address-tests.rs`, and this step changes the `synthetic_section` signature that file's `sibling` helper calls.
- Pieces: the `build_sections` split (D1-5) and the `synthetic_section` trim (C-1), built jointly. They share no code, but both are parser-crate edits covered by the same parser and engine test runs.
- Paths are relative to `crates/promptforge-internal/`, as in the Technical Design.
- `parser/src/build.rs`: `build_sections` keeps its signature and behavior. Move its four inline checks into private helpers that produce today's exact error kinds, spans, and message texts, so `build_sections` reads as the sequence of steps:
  - `fn check_child_level(heading: &Heading, parent_level: u8) -> Result<()>`: the orphan-heading error
  - `fn section_name(heading: &Heading, level: u8) -> Result<String>`: the empty-heading error, returning the name
  - `fn list_items(blocks: &[Block], name: &str) -> Result<Vec<String>>`: the list-only detection and its `parse_bullet_items` call
  - `fn check_unique_sibling(siblings: &[(String, u32)], name: &str, heading: &Heading, heading_abs_line: u32) -> Result<()>`: the duplicate-sibling error naming both lines
- `parser/src/test_support.rs`: `synthetic_section(name: &str, level: u8, blocks: Vec<Block>) -> Section` sets `items: Vec::new()`, and its doc drops "a list of items".
- Call sites drop the trailing `Vec::new()`: the `sibling` helper in `engine/src/heading_address-tests.rs` and the three calls in `engine/src/execute/tests/exec_flow.rs::duplicate_top_level_section_names_error_loudly`.
- Tests: no test or assertion changes. The orphan, empty-heading, duplicate-sibling, list, and line-mapping tests in `parser/src/tests/headings.rs`, `parser/src/tests/lists.rs`, `parser/src/tests.rs`, and `parser/src/build-tests.rs` pin every message, kind, and span; for C-1 the build is the check.
- Verify: `cargo nextest run --locked -p promptforge-parser --all-features` and `cargo nextest run --locked -p promptforge-engine --all-features` pass.
- Commit: one `promptforge` commit with the parser edits and the four call sites.

</step-1>

<step-2>

### Step 2: Give H1's visible set one helper and rename resolve_sibling to resolve_visible [completed]

- Component: Engine scope module
- Placement: second, after Parser cleanup, because both edit `engine/src/heading_address-tests.rs` (the `sibling` helper there, then the renamed tests), so they run in sequence in one checkout.
- Pieces: `h1_visible_sections` (D1-14) and the `resolve_visible` rename (D1-15), built jointly: both edit `execute/walk_target.rs`, `execute/scheduler/walk.rs`, and `execute/scheduler/h1.rs`, and the engine suite covers both.
- Paths are relative to `crates/promptforge-internal/engine/src/`.
- `execute/walk_target.rs`: add `pub(super) fn h1_visible_sections(prompt: &Prompt) -> &[Section]` beside `visible_sections`, returning the whole top-level slice from `promptforge_parser::detail::sections` with the doc comment given in the Technical Design. Import `crate::parser::Prompt` next to the existing `crate::parser::Section`.
- Call `h1_visible_sections` at the three H1 sites and delete each one's duplicated "section 0 excludes nothing" comment: `execute/section_context-construct.rs::new_live_h1` (keeps its `.to_vec()`, since the closure owns its set), `execute/scheduler/walk.rs::resolve_chain_target`'s H1 branch, and `execute/scheduler/h1.rs::end_live_h1_at_jump`. Leave the other `detail::sections` uses, which are not visible sets: `execute/scheduler.rs` (`SlicePath::resolve`), `execute/context.rs` (the section count), and the emptiness checks in `execute/scheduler/drive.rs` and near line 101 of `execute/scheduler/h1.rs`.
- `heading_address.rs`: rename `resolve_sibling` to `resolve_visible` with the same signature, body, and messages. Update every mention: `heading_address.rs` itself (2), `execute/walk_target.rs` (2), `execute/scheduler/walk.rs` (2), `execute/scheduler/h1.rs` (1), and `heading_address-tests.rs` (15, including the seven `resolve_sibling_*` test names, which become `resolve_visible_*`). That test file's module doc becomes "Tests for heading lookup over a visible set."
- Tests: bodies and assertions stay; only the seven test names and the module doc change. The H1 suites stay unchanged: `execute/tests/suite/exec_flow/h1_control.rs`, `execute/tests/suite/exec_flow/prompt_reads.rs::prompt_reads_work_in_h1_and_the_shared_library`, and `execute/tests/scheduler/live_h1/`.
- Verify:
  - `rg -n resolve_sibling crates` prints nothing.
  - `rg -n "detail::sections\(" crates/promptforge-internal/engine/src --glob "!**/tests/**" --glob "!*-tests.rs"` lists exactly five sites: `h1_visible_sections` in `walk_target.rs`, `scheduler.rs`, `context.rs`, `scheduler/drive.rs`, and the emptiness check in `scheduler/h1.rs`.
  - `cargo nextest run --locked -p promptforge-engine --all-features` passes, including the `resolve_visible_*` tests and `find_visible_returns_none_for_an_address_not_in_the_set`.
- Commit: one `promptforge` commit.

</step-2>

<step-3>

### Step 3: Mark closed plans' todos completed, going forward and across vibe/ history

- Component: Plan history
- Placement: third. It depends on no other component. It follows the code components so the 133-file data edit stays out of the `[WIP] Plan:` seed commit that Step 1 amends, and it precedes the last step so that step's exit gate runs once after every work item lands.
- Pieces: the vibe coder close step (forward) and the `vibe/` history flip (backward), built jointly because they are the two halves of D1-16 and share one set of checks. Inside the step, the `tools-public` commit comes first, then the `promptforge` commit.
- `c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md`:
  - Run Sequence step 5, today "Commit the deletion of `vibe/ACTIVE` with subject `Close plan: <words>`, a blank line, and the plan's `Plan:` trailer.", becomes "Set every frontmatter todo `status:` in <PLAN PATH> to `completed`, then commit that change with the deletion of `vibe/ACTIVE` with subject `Close plan: <words>`, a blank line, and the plan's `Plan:` trailer."
  - Append after the existing `*2026-09-08 - GPT-5.6 Sol (Cursor agent)*` line, separated from it by one blank line: `*2026-10-10 - Claude Opus 5.5 (Cursor agent). Close step marks the plan copy's todos completed.*`
  - The coding sub-agent leaves the file unstaged. The session commits `planning/vibe-coder.md` alone in `tools-public` after the sub-agent returns and before this step's `promptforge` commit, and amends that commit after any later round that changes the file. Nothing is pushed.
- `vibe/` history flip, a one-off session data edit with no committed tooling:
  - Line endings before the edit, which the edit must leave as they are: every tracked `vibe/` file is `i/lf` except `vibe/2026-09/2026-09-26-4-store-into-vfs.md` (`i/-text`), and only `vibe/.cursorignore` and `vibe/2026-10-09-3-models-loop-in-rust.md` are `w/crlf`.
  - In every tracked `vibe/**/*.md` plan except the one `vibe/ACTIVE` names (today 133 files have open todos, 5 of them with `in_progress`), within the YAML frontmatter only (line 1 `---` through the next line that is exactly `---` apart from its terminator), lines matching `^(\s+)status: (pending|in_progress)\s*$` become `$1status: completed`.
  - Edit bytes line by line and keep each line's own terminator: `vibe/2026-10-09-3-models-loop-in-rust.md` is CRLF in the working tree, so the trailing `\s*` must not swallow its `\r`. Keep each file's final newline and change no other byte, including the two lone CR bytes in `vibe/2026-09/2026-09-26-4-store-into-vfs.md`.
- Close requirement: the flip skips this plan's own copy. The run's close commit sets that copy's frontmatter todos to `completed` before deleting `vibe/ACTIVE`, whatever close-step version the session read.
- Verify:
  - `git grep -n -E "^\s+status: (pending|in_progress)\s*$" -- vibe` lists only the active plan copy's lines.
  - `git diff --text --numstat -- vibe` shows equal added and deleted counts per file, and every added or removed line in `git diff --text -U0 -- vibe`, apart from the `---` and `+++` file headers, matches `^[-+]\s+status: `. The `--text` flag is required: git classifies `vibe/2026-09/2026-09-26-4-store-into-vfs.md` as binary because of its lone CR bytes, so without it numstat prints `-` for that file and the line check cannot see its changes.
  - `git ls-files --eol vibe` still shows every tracked file `i/lf` except `vibe/2026-09/2026-09-26-4-store-into-vfs.md` (`i/-text`), with `w/crlf` only on `vibe/.cursorignore` and `vibe/2026-10-09-3-models-loop-in-rust.md`.
  - `rg -n -F "Set every frontmatter todo" c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md` finds exactly one line, in Run Sequence step 5.
- Commit: the `tools-public` commit of `planning/vibe-coder.md` alone, then one `promptforge` commit with the flipped `vibe/` files.

</step-3>

<step-4>

### Step 4: Correct the fanout-arm test name, the reserved-alias docs, and the spec status lines, then run the exit gate

- Component: Names and docs
- Placement: last. It depends on no other component, and the exit gate attached to it runs once after every work item lands, so any gate fix goes into a commit that is not yet finalized.
- Pieces: the test rename (C-2), the doc corrections (C-3), and the spec status lines (X-1), built jointly: three independent text edits covered by one engine run, the docs build, and two `rg` checks.
- Paths are relative to `crates/promptforge-internal/` unless they name a sibling repository.
- `engine/src/execute/tests/suite/exec_flow/fanout_arms.rs`: rename `call_and_list_inside_a_fanout_arm_reject_non_visible_sections` to `call_raises_and_list_reads_nil_for_non_visible_sections_in_a_fanout_arm`; body and doc comment stay.
- `lua/src/globals.rs` module doc (lines 29 to 33): the sentence "A frontmatter tool alias installs as a global of its own name, so the parser refuses one that is reserved" becomes "A Plugin prelude global may not take one."
- `parser/src/parse.rs` `# Errors` (line 32): drop "a tool alias is a reserved name," from the `Frontmatter` list.
- `c:\Users\Vinnie\cursor\promptforge-design\design\architect-promptforge-prompting-language.md` lines 364 and 370: both read `**Implementation status:** Active.` The coding sub-agent leaves the file unstaged. The session commits it alone in `promptforge-design`, staging only that file, after the sub-agent returns and before this step's `promptforge` commit, and amends that commit after any later round that changes it. Nothing is pushed.
- Verify:
  - `cargo nextest run --locked -p promptforge-engine --all-features call_raises_and_list_reads_nil` runs one test and passes.
  - `rg -n "tool alias is a reserved|parser refuses one that is reserved" crates` prints nothing.
  - `rg -n "Active: new in this plan|now .nil. on a miss" ../promptforge-design/design/architect-promptforge-prompting-language.md` prints nothing, and lines 364 and 370 read `**Implementation status:** Active.`
- Exit gate, run once from the `promptforge` root after this step's edits; fixes land in this step's commit:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - With `CARGO_BUILD_WARNINGS=deny` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`): `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - With `RUSTDOCFLAGS="-D warnings"`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check` (or the nightly `crates/build-xtask/src/api/toolchain.rs` names), which confirms `crates/promptforge/public-api.txt` is unchanged, then `cargo test -p build-xtask`
  - `node --test test/docs-claims.mjs` from `crates/workshop/ui`
  - Rerun this step's `rg` checks, Step 2's two `rg` checks, and Step 3's `git grep` check.
- Commit: the `promptforge-design` commit of the spec file alone, then one `promptforge` commit.

</step-4>

</execution-plan>
