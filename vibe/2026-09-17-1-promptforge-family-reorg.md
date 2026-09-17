---
name: Promptforge family reorganization
overview: Rename promptforge-api to promptforge-api-runtime, rename shared-promptforge-api to promptforge-api-types, move the 9 private promptforge crates into a private crates/promptforge/ container (directories drop the prefix, package names unchanged), re-export the types crate through the runtime, and add a position-based privacy rule to build-xtask. One commit.
todos:
  - id: single-commit
    content: "One commit: move, rename, re-export, privacy rule, docs, verification"
    status: pending
isProject: false
---

# Promptforge Family Reorganization

<product-contract>

## Product Requirements

The workspace at `crates/` holds 46 flat sibling crates, and the PromptForge family's public/private split exists only as a name-prefix convention checked by `build-xtask`. This plan makes the split a property of the directory tree: two public crates at the root of `crates/`, nine private crates under a `crates/promptforge/` container, and a positional check that fails the build when anything outside the container depends into it. The change lands as exactly one commit.

- Problem and users: Maintainers and coding agents browsing `crates/` cannot tell from the tree which PromptForge crates are public API and which are internal machinery; the distinction lives in `crates/build-xtask/src/product.rs` (the single-entry rule at lines 100-101) and in prose at `AGENTS.md` line 27. The 46-sibling flat directory is hard to scan.
- Goals:
  - PromptForge's entire public surface is two root-level crates: `promptforge-api-runtime` (behavior: `run`, `Environment`, `Prompt`, `RunContext`) and `promptforge-api-types` (vocabulary: observer, catalogs, tool contract, events, cancel, untrusted, wire).
  - The nine private crates (`promptforge-lua`, `promptforge-parser`, `promptforge-store`, `promptforge-vfs`, `promptforge-model-client`, `promptforge-tool-picker`, `promptforge-web`, `promptforge-webfetch`, `promptforge-web-search`) live at `crates/promptforge/<short-name>/`, directories dropping the `promptforge-` prefix, package names unchanged.
  - `cargo test -p build-xtask` fails when any crate outside `crates/promptforge/` other than `promptforge-api-runtime` depends on a crate inside it.
  - A CLI agent author depends on `promptforge-api-runtime` alone and can name the observer, catalog, and tool types through it.
- Non-goals: flattening `src/` directories; moving or renaming gateway or workshop crates; relocating `shared-cloud-providers`; renaming the nine private packages; encoding intra-family layering in the tree.
- Success criteria: every repository gate passes (see Testing Plan); the new privacy check reports a fixture crate outside the container that depends into it and accepts the four legal shapes; the runtime crate's doc example compiles with the runtime as its only PromptForge dependency; `ls crates/` shows `promptforge-api-runtime`, `promptforge-api-types`, and `promptforge/` and no other `promptforge-*` entry.
- Constraints:
  - One commit. No intermediate commits, no provisional commits left in history.
  - Package names of the nine moved crates do not change, so no `use promptforge_lua::` site changes and `.config/nextest.toml`'s `package(promptforge-tool-picker)` filter keeps matching.
  - Cargo errors on a glob-matched member directory without a manifest, so `crates/promptforge` must be listed in the root `exclude` (the same reason `crates/shared-ui` is excluded at `Cargo.toml` line 6).
  - Repository policy (`AGENTS.md`, Engineering, "Repository policy binds plans") forbids a plan from adding a topology check without explicit user approval. The user approved the privacy check during planning. The commit message records this.
- Open questions: None.

## Functional Specification

Three actors interact with the result: an outside crate author who consumes PromptForge, an inside crate author who works on the machinery, and the structural harness that checks every manifest. The observable behavior is which dependency edges compile without a harness violation and which the harness reports.

- Actors and workflows:
  - Outside author (workshop crates, a future CLI): adds `promptforge-api-runtime` for behavior; may add `promptforge-api-types` directly when only vocabulary is needed and the runtime's compile cost (Lua VM, HTTP stacks, tokio) is unwanted, as `workshop-protocol` and `workshop-gateway` do today (`crates/workshop-protocol/Cargo.toml` line 12, `crates/workshop-gateway/Cargo.toml` line 17). Reaches types through `promptforge_api_runtime::types::...` or `promptforge_api_types::...`.
  - Inside author: crates under `crates/promptforge/` depend on each other and on `promptforge-api-types` and `shared-*` freely; never on `promptforge-api-runtime` (existing direction, `crates/promptforge-api/AGENTS.md` line 8).
  - Harness: `cargo test -p build-xtask` walks `crates/` recursively, classifies each manifest by package name and by directory position, and reports violations.
- Inputs and outputs: input is every `Cargo.toml` under `crates/` (all dependency tables: normal, dev, build, target-specific); output is a list of violation strings, empty on success.
- States and validation: a directory directly under `crates/` with no `Cargo.toml` is a container; a directory with a `Cargo.toml` is a crate and the walker does not descend into it. A dependency edge into a container crate is legal when the dependent is in the same container or is that container's named public crate (`promptforge-api-runtime` for `crates/promptforge/`).
- Errors and recovery: a violation reads `<package> depends on <dep>: crates/promptforge is private to its family; only promptforge-api-runtime may depend into it`. Recovery is moving the needed type into `promptforge-api-types` or calling through `promptforge-api-runtime`.
- Security and privacy behavior: None beyond the dependency boundary; no runtime behavior changes.
- Acceptance criteria:
  - `crates/promptforge/` contains exactly nine crate directories and no `Cargo.toml` of its own.
  - `promptforge_api_runtime::types::observe::NullObserver` resolves from an outside crate with only `promptforge-api-runtime` declared.
  - The harness's fixture test for an outside crate depending on a container crate fails; fixture tests for inside-to-inside, runtime-to-inside, and outside-to-runtime pass.
  - No file outside `vibe/` names `shared-promptforge-api`, `shared_promptforge_api`, or the bare `promptforge-api` / `promptforge_api` crate identifier.

</product-contract>
<implementation-contract>

## Technical Design

The tree becomes the boundary and the harness reads the tree. Two renames turn the existing vocabulary and runtime crates into `promptforge-api-types` and `promptforge-api-runtime`; nine moves put the machinery under a manifestless container; one re-export line gives outside authors a single dependency; one new harness rule makes the container private with a single named exception. No `.rs` module file moves relative to its own crate.

- Architecture:
  - Target tree: `crates/promptforge-api-runtime/` (was `crates/promptforge-api/`), `crates/promptforge-api-types/` (was `crates/shared-promptforge-api/`), `crates/promptforge/{lua,parser,store,vfs,model-client,tool-picker,web,webfetch,web-search}/`.
  - Dependency direction is unchanged: runtime depends on all nine private crates and on types; private crates depend on types and `shared-*`; outside crates depend on runtime and types only.
  - The Cargo dependency model makes this sound without further work: a crate can name only its direct dependencies, so outside crates cannot reach `promptforge_lua::` through the runtime, and rustc rejects any private type appearing in the runtime's public signatures. Every public item in the runtime is defined locally or re-exported from the types crate or `shared-vfs` (`crates/promptforge-api/src/lib.rs` lines 92-99).
- Modules and interfaces:
  - `crates/promptforge-api-runtime/src/lib.rs` gains `pub use promptforge_api_types as types;`. A per-module flat re-export is impossible because the runtime already has modules named `capabilities`, `observe`, `tools`, `untrusted`, `cancel` (lines 71-87) that collide with the types crate's modules.
  - `crates/build-xtask/src/product.rs`:
    - `workspace_crates` (line 110) records each crate's manifest directory alongside package name and dependency list, and walks `crates/` recursively: a directory with `Cargo.toml` is a crate (no descent); otherwise descend one level.
    - `boundary_breach` (line 79) gains a path-aware container rule: when the dependency's directory is under `crates/<container>/` where `<container>` has no manifest, the dependent must be under the same container or be the container's named public crate; `crates/promptforge/` maps to `promptforge-api-runtime`.
    - The single-entry exception at line 100 becomes the set `{promptforge-api-runtime, promptforge-api-types}`; both new names start with `promptforge-` so `family()` (line 36) classifies them `Family::Promptforge` unchanged, and without the widened exception every workshop consumer of the types crate would violate.
    - Doc comment lines 12-13 and fixture strings at lines 315-337 use the new names; `write_crate` (line 227) gains a directory-path parameter or a `write_nested_crate` sibling.
  - `crates/build-xtask/src/tidy.rs` `participating_crates` (line 211) uses the same recursive walk so the marker scan keeps seeing every crate. Tier rules there collect `workshop-*` deps only (lines 128-138) and resolve workshop crates at `crates/<name>` (line 74); unaffected.
- File and public API changes:
  - Moves: `git mv` the nine private crates to `crates/promptforge/<short-name>/`.
  - Renames: `git mv crates/shared-promptforge-api crates/promptforge-api-types`, package `name = "promptforge-api-types"`; `git mv crates/promptforge-api crates/promptforge-api-runtime`, package `name = "promptforge-api-runtime"`.
  - Root `Cargo.toml`: `members = ["crates/*", "crates/promptforge/*"]`; `exclude` adds `"crates/promptforge"`; `[workspace.dependencies]` keys and paths at lines 22-23 (renames) and lines 36-49 (nine moved paths).
  - Member manifests: `shared-promptforge-api.workspace = true` becomes `promptforge-api-types.workspace = true` in workshop-server (line 69), workshop-sessions (20), workshop-gateway (17), workshop-protocol (12), the runtime (17), and the nine private crates; `promptforge-api.workspace = true` becomes `promptforge-api-runtime.workspace = true` in workshop-server (68) and workshop-sessions (19).
  - Identifier sweeps over every `.rs` including doc tests, benches, and tests: `shared_promptforge_api` to `promptforge_api_types` (about 90 files); `\bpromptforge_api\b` to `promptforge_api_runtime` with a word boundary so the already-renamed types identifier is untouched (hits: runtime `lib.rs` doc examples lines 30 and 52, `benches/models_loop.rs`, `tests/suite/*`, workshop-sessions sources, workshop-server tests).
  - Kebab prose sweep `shared-promptforge-api` to `promptforge-api-types` and `promptforge-api` to `promptforge-api-runtime` outside `vibe/`: runtime `lib.rs` line 10; the types crate's own `AGENTS.md` and `README.md` titles; `crates/promptforge-web-search/AGENTS.md` line 5; `crates/promptforge-webfetch/AGENTS.md` line 7; `crates/promptforge-model-client/AGENTS.md` line 7 and `README.md` line 24; code samples in `crates/promptforge-web/README.md` lines 10 and 14, `crates/promptforge-webfetch/README.md` line 18, `crates/promptforge-web-search/README.md` line 18 (none is `include_str!`-doc-tested; accuracy only); `crates/promptforge-api/AGENTS.md` title, line 5, and line 9 rewritten to describe the container instead of listing internal crates; `crates/promptforge-api/README.md` title, badges, and usage example rewritten to the one-dependency form with `promptforge_api_runtime::types::observe::NullObserver`; `crates/workshop-server/README.md` lines 5 and 145; `crates/workshop-sessions/README.md` lines 3 and 9; `tools/document.md` line 123 (line 132 names a nonexistent `promptforge-agent` crate and stays as is).
  - Guide: edit the source `guide/src/agent/01-agent-programs.md` line 53, then run `cargo run -p build-user-guide` to regenerate `guide/promptforge-agent-guide.md` and `guide/src/SUMMARY.md`; never hand-edit the assembled file.
  - Path literals: `.github/actions/hf-model-cache/action.yml` line 28 to `hashFiles('crates/promptforge/tool-picker/build.rs')`; Lua chunk name at `crates/promptforge-lua/src/messages.rs` line 28 to `@crates/promptforge/lua/src/__impl_messages.lua`; chunk name at `crates/promptforge-lua/src/coro.rs` line 23 to `@crates/promptforge-api-runtime/src/lua/__impl_coro.lua` with the four asserts in `crates/promptforge-api/src/lua-coro-tests.rs` lines 408, 421, 539, 548.
  - `AGENTS.md` (root): line 27 names both public crates and adds that crates under `crates/promptforge/` are private, with `promptforge-api-runtime` the only outside crate permitted to depend into them; line 29 gains a companion sentence naming PromptForge's public surface now that the types crate has left `shared-*`; line 56 lists the privacy rule among what `cargo test -p build-xtask` enforces.
  - `Cargo.lock` regenerates with the two new package names on first build and is part of the commit.
- Data, persistence, failure, security, and privacy constraints: nothing persisted or on the wire changes; the only failure mode introduced is a harness test failure with the violation text above; `[workspace.lints]` (`missing_docs`, `unreachable_pub`) do not fire on a crate-level `pub use` re-export.

</implementation-contract>
<verification-contract>

## Testing Plan

Verification is the repository's existing gate set plus four new fixture tests for the privacy rule. Because the change is one commit, every gate runs once against the final tree.

- Unit: in `crates/build-xtask/src/product.rs`, fixture tests on a temp workspace: an outside crate depending on a crate under `crates/promptforge/` is reported with the privacy message; a crate under the container depending on a sibling passes; `promptforge-api-runtime` depending into the container passes; an outside crate depending on root-level `promptforge-api-runtime` or `promptforge-api-types` passes. Existing single-entry fixtures updated to the new names still pass.
- Integration and end-to-end: `cargo check --workspace --all-targets`; `cargo nextest run --locked -p promptforge-api-runtime -p promptforge-api-types -p promptforge-lua -p promptforge-parser -p workshop-sessions -p workshop-gateway -p workshop-protocol`; `cargo nextest run --locked -p workshop-server --features headless`; `cargo test --doc -p promptforge-api-runtime -p promptforge-api-types` (the runtime's doc example must compile in the one-dependency form).
- Regression, security, and performance: `cargo test -p build-xtask`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and the workshop variant; `cargo fmt --all --check`; `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; `mdbook build guide`; `cargo deny check` when installed. No performance change expected: a re-export generates no code.
- Exit criteria: all commands above exit zero; `rg 'shared-promptforge-api|shared_promptforge_api|\bpromptforge_api\b' --glob '!vibe/**' --glob '!target/**'` returns nothing; `git status` clean after the single commit.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Two public crates named `promptforge-api-runtime` and `promptforge-api-types`, private crates under `crates/promptforge/` with the prefix dropped from directories. Rationale: the tree shows the public/private split; the family already has exactly two crates that outsiders consume. User's words: "promptforge-api-runtime # the runtime (execute:run) / promptforge-api-types # lightweight types / promptforge/ # the rest of the promptforge crates. remote promptforge- prefix".
  - Package names of the nine moved crates unchanged. Rationale: no `use` sites change, `cargo -p` names stay stable, and a bare package `lua` would collide with the runtime's existing `mod lua`. User selected "Keep package names" when asked.
  - Privacy rule is positional with one named exception (`promptforge-api-runtime`). Rationale: the runtime must depend on the private crates to implement `run`; a rule without the exception makes the family's own public crate illegal. User selected "Named exception" when asked.
  - Enforcement includes the new privacy check, not only walker fixes. User selected "Minimal fixes plus the privacy rule" when asked; this is the approved policy exception the repository requires for a new topology check.
  - Types crate re-exported through the runtime as `pub use promptforge_api_types as types;`. Rationale: a CLI author asked "do they just add promptforge-api as a dependency and nothing else?" and today the answer is no; the alias form is forced by module-name collisions. Recommended during planning and accepted into the plan without objection.
  - The three product families are peers; workshop is never nested under promptforge. User's words: "why do you keep showing workshop under promptforge/ stop doing that".
  - One commit. User's words: "I want it all in one commit ... ONLY ONE COMMIT. JUST ONE."
- Rejected alternatives:
  - Pure re-export facade crate at the root with the runtime moved inside the container. Reason: extra crate, feature forwarding, doc-inline chores, and the shim still needs the same named exception. Revisit if a second public behavior crate appears.
  - `crates/promptforge/internal/` subdirectory with a purely positional rule. Reason: mixes public and private at one level and removes the runtime from `ls crates/`. Revisit if the named exception list grows past two entries.
  - Renaming the nine packages to drop the prefix. Reason: every `use` site changes and `mod lua` in the runtime collides with an extern crate `lua`. Revisit never unless the runtime's module layout changes.
  - Header/implementation split across directory trees with `#[path]`. Reason: fights rust-analyzer and contributor expectations; `lib.rs` plus `cargo doc` already give the API view. Not revisited.
  - Nesting workshop under promptforge to satisfy an ancestor-visibility rule. Reason: user rejected; the rule was wrong, not the tree.
- Assumptions, risks, and notes:
  - `tools-public/rulebooks/rust-crate-rulebook.md` section 2 prescribes a flat `crates/` with directory equal to package name; this plan deviates on purpose so the tree encodes the boundary. That rulebook is a reference, not a repository rule.
  - `.config/nextest.toml` heavy-group filter matches by package name; unaffected.
  - `crates/build-xtask/src/new_crate.rs` scaffolds `workshop-*` crates at `crates/<name>`; unaffected.
  - `crates/build-user-guide` walks `guide/src/<set>/`, not `crates/`; unaffected except for the regeneration step.
  - `.cursor/rules/*.mdc` globs, `.gitignore`, `.gitattributes`, `dist-workspace.toml`, `deny.toml`, root `README.md`, and CI workflows reference only gateway, workshop, and UI paths, except the `hf-model-cache` action already listed. `guide/scratch/` and `guide/book/` are gitignored generated output.
  - `vibe/` plan logs reference old paths; historical, intentionally not rewritten.
  - Risk: the `\bpromptforge_api\b` sweep must run after the `shared_promptforge_api` sweep and must exclude `promptforge_api_types`; a plain substring replace would corrupt the types identifier.
  - Risk: forgetting `exclude = ["crates/promptforge"]` produces a Cargo error about a missing manifest for the container.

### Deferred and Out of Scope

- Deferred: flattening `src/` directories; revisit after this commit lands and the tree shape has settled.
- Deferred: applying the same public/private container shape to the gateway family (`gateway-api`, `gateway-api-discovery` public; `crates/gateway/*` private); revisit once the promptforge shape has been lived with.
- Deferred: relocating `shared-cloud-providers` (a standalone tool depending only on `shared-gateway-api`, with no consumers) into the gateway family; revisit with the gateway move.
- Out of scope: moving or renaming any workshop crate; encoding intra-family tiers in the tree; changes to the SPA or UI packages.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` (default member is `crates/gateway` only; full desktop app via `cargo workshop`, low-level `cargo build -p workshop` after sidecar staging; headless gateway check `cargo check -p gateway --no-default-features`).
- Focused test command pattern: `cargo nextest run --locked -p <crate>` for unit and `tests/` targets; `cargo test --locked -p <crate> --test it <test_name_substring>` for one integration test (crates whose `tests/it/main.rs` is the single integration target); `cargo test -p <crate> --doc` for doctests (nextest skips doctests).
- Component test command pattern: `cargo nextest run --locked -p <crate-a> -p <crate-b> --all-features`; workshop partition is `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` plus `cargo nextest run --locked -p workshop-server --features headless` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`; UI packages via `npm test` in `crates/workshop-server/ui` and `crates/gateway-config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the workshop partition above, then `cargo test -p build-xtask` (boundary and structural harness) and `cargo test -p gateway-stt --test it architecture` (product dependency boundaries).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop partition `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; supply chain `cargo deny check` and `cargo audit`; on-demand structural report `cargo xtask tidy`.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook in `.githooks/`; `rustfmt.toml` sets `style_edition = "2024"`).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; user guide `mdbook build guide`.
- Test placement and naming conventions: unit tests are inline `#[cfg(test)] mod tests` blocks in nearly every source file, and when they outgrow the file they move to a kebab sibling `foo-tests.rs` wired with `#[path = "foo-tests.rs"] mod tests;` (examples: `promptforge-api/src/tools-tests.rs`, `promptforge-lua/src/messages-tests.rs`, `promptforge-tool-picker/src/picker-tests.rs`) or a crate-level `src/tests.rs` (`promptforge-lua`, `promptforge-parser`, `promptforge-store`); shared unit helpers live in `src/test_support.rs` or `src/test_fixtures.rs`, sometimes behind a `test-fixtures` Cargo feature. Integration tests use a single target `tests/it/main.rs` with one module per concern (`tests/it/boot.rs`, `tests/it/chat.rs`, and a `support.rs` helper module), sometimes with a sibling `tests/common/` or `tests/fixtures/` directory; `promptforge-api` instead uses `tests/suite/main.rs` plus a `tests/prompts/{valid,invalid,execution}/*.md` corpus. Benches live in `benches/` (`promptforge-api/benches/models_loop.rs`, `promptforge-lua/benches/surface.rs`) using criterion. Test names are long descriptive snake_case sentences (`a_direct_launch_recovers_the_lease_from_a_terminated_owner`). Nextest config in `.config/nextest.toml` puts `promptforge-tool-picker`, `gateway-stt`, and `gateway-stt-backend-whisper` in a `heavy` group capped at 2 threads. `clippy.toml` allows `unwrap`/`expect` in tests only.
- Directory map: `Cargo.toml` (workspace, `resolver = "3"`, `members = ["crates/*"]`, `exclude = ["crates/shared-ui"]`, `default-members = ["crates/gateway"]`, all internal crates declared in `[workspace.dependencies]` at path + version, shared `[workspace.lints]`); `crates/` holds 47 directories in five name families: `promptforge-*` (11: api, lua, model-client, parser, store, tool-picker, vfs, web, web-search, webfetch), `gateway-*` (12: gateway, config, config-ui, local, logging, protocol, routing, stt, stt-backend-whisper, stt-engine, web-search, whisper-ffi), `workshop-*` (13: workshop, gateway, menu, protocol, registry, server, server-api, sessions, status, support, user-state, workspace), `shared-*` (9: cloud-providers, gateway-api, gateway-discovery, loopback, progress, promptforge-api, ui [TypeScript+CSS, not a Rust crate], vfs), `build-*` (5: llama-cuda, ui, user-guide, workshop, xtask); `crates/workshop-server/ui/` and `crates/gateway-config-ui/ui/` are npm packages (esbuild, `npm run typecheck|build|test`); `guide/` is the mdbook user guide; `prompts/` holds shipped prompt files; `tools/` holds Node scripts (`stage-gateway-sidecar.mjs`, `gateway-tts-live.mjs`) with `.test.mjs` siblings; `vibe/` holds `archdoc.md` and dated plan/debt records; `.github/workflows/` (ci.yml plus release, nightly, guide, miri, installer workflows); `.githooks/` (pre-commit fmt, pre-push clippy + headless gateway check + cargo deny); `.cargo/config.toml` (aliases `cargo workshop` -> `run -p build-workshop --`, `cargo xtask` -> `run -p build-xtask --`; static CRT on Windows MSVC); `.config/nextest.toml`; `rust-toolchain.toml` (stable), `rustfmt.toml`, `clippy.toml`, `deny.toml`, `dist-workspace.toml`; `AGENTS.md` is the binding repository policy; `target/` and `target-msrv/` are build outputs.
- Component boundaries: three products plus two support families, dependency rules binding normal, dev, build, and target-specific deps alike (AGENTS.md, enforced by `cargo test -p build-xtask`). `shared-*` depends on no product crates and is the cross-product API surface (progress, loopback, protocol, gateway discovery, VFS substrate `shared-vfs`, cloud provider sheets, `shared-promptforge-api`, `shared-gateway-api`). `promptforge-*` depends only on `shared-*` and other `promptforge-*` crates, never on gateway or workshop; `promptforge-api` is the single entry point: crates outside the family may depend only on `promptforge-api`, never on `promptforge-lua`, `promptforge-parser`, `promptforge-store`, `promptforge-vfs`, `promptforge-model-client`, `promptforge-tool-picker`, `promptforge-web*`, or `promptforge-webfetch`. `gateway-*` depends only on `shared-*` and other `gateway-*`, never on promptforge or workshop; the `gateway` binary (`promptforge-gateway`) owns model routing, provider credentials, and local inference lifecycle (archdoc A1, A2, A5). `workshop-*` depends on promptforge (via `promptforge-api`), shared, and other `workshop-*`, never on gateway crates; internal tiers flow shell -> features -> services -> vocabulary, the Tauri shell `workshop` depends on `workshop-server-api` and never `workshop-server`, and every workshop-* `lib.rs` opens with a `//!` doc containing a `## Invariants` marker naming allowed and forbidden dependencies. `build-*` crates are tooling for specific outputs (`build-xtask` depends on no workspace crates). Per archdoc: executor (`promptforge-api` + `promptforge-lua` + `promptforge-parser`) -> gateway protocol, store, Lua VM boundary, shared substrate; store (`promptforge-store`, exposed as `vfs.store(&access)`) -> VFS layer (`shared-vfs` backends, `promptforge-vfs` policy gate).
- Conventions summary: Rust edition 2024, stable toolchain, `unsafe_code = "forbid"` workspace-wide (gateway-stt crates verified with `-F unsafe-code`; only `gateway-whisper-ffi` is the owned unsafe boundary with documented safety invariants), `missing_docs`, `unreachable_pub`, `missing_debug_implementations` warned, clippy `all` denied and `pedantic` warned, `unwrap_used`/`expect_used` denied outside tests, rustdoc broken and private intra-doc links denied; every crate inherits `[lints] workspace = true`. Every `lib.rs` opens with a `//!` crate doc. No file exceeds 500 lines (split first, then edit). Source directories are flat by default: a submodule group of one or two files lives as kebab siblings `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`; three or more files rehydrate into a `foo/` directory in standard module layout; `tests/` and `benches/` are exempt. Comments explain non-obvious constraints and every external-bug workaround cites its upstream issue URL; dependency pins in `Cargo.toml` have a comment explaining the pin. Cargo features gate real constraints (toolchain, native build, `headless`, `test-fixtures`), never product shape; runtime and serve paths never compile native deps or install process-global state; errors are written for model consumption (concise, required versus actual). Behavior changes ship with tests in the same change; structural enforcement (parsers, snapshots, allowlists, counts, ceilings, topology checks) requires explicit user approval. Long-running work reports through `shared-progress`. Internal crate versions are `0.3.0` for released crates and `0.0.0` for the newer workshop-* decomposition crates. Repository policy in `AGENTS.md` binds plans; `vibe/archdoc.md` lists invariants A1-A9. Commits pass `cargo fmt --all --check` (pre-commit) and clippy + headless gateway check (pre-push); CI requires a clean tree after builds (no build step writes into the repo). SPA rules: CSS beside its TypeScript, `--ws-*` tokens only, no `localStorage` (persist via `ui-storage` adapter to `workshop-user-state` or the `.pfwork` workspace file).

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Reorganize the PromptForge family into two public crates and a private container [completed]

- Component: `none`
- Objective: make the public/private split of the PromptForge family a property of the `crates/` tree, enforced by `build-xtask`, landing as exactly one commit. No intermediate or provisional commits: stage everything and commit once after the full verification list below passes.
- Tree moves and renames (all via `git mv`):
  - `crates/shared-promptforge-api` -> `crates/promptforge-api-types`; set `name = "promptforge-api-types"` in its `Cargo.toml`.
  - `crates/promptforge-api` -> `crates/promptforge-api-runtime`; set `name = "promptforge-api-runtime"` in its `Cargo.toml`.
  - `crates/promptforge-lua`, `promptforge-parser`, `promptforge-store`, `promptforge-vfs`, `promptforge-model-client`, `promptforge-tool-picker`, `promptforge-web`, `promptforge-webfetch`, `promptforge-web-search` -> `crates/promptforge/{lua,parser,store,vfs,model-client,tool-picker,web,webfetch,web-search}/`. Package names unchanged. `crates/promptforge/` gets no `Cargo.toml` of its own.
- Root `Cargo.toml`:
  - `members = ["crates/*", "crates/promptforge/*"]`; `exclude` adds `"crates/promptforge"` beside `"crates/shared-ui"` (omitting it produces a Cargo missing-manifest error for the container).
  - `[workspace.dependencies]`: rename keys `shared-promptforge-api` -> `promptforge-api-types` and `promptforge-api` -> `promptforge-api-runtime` with their new paths; update the nine moved crates' `path` values to `crates/promptforge/<short-name>`.
- Member manifests (normal, dev, build, and target-specific tables alike):
  - `shared-promptforge-api.workspace = true` -> `promptforge-api-types.workspace = true` in `workshop-server`, `workshop-sessions`, `workshop-gateway`, `workshop-protocol`, `promptforge-api-runtime`, and the nine private crates.
  - `promptforge-api.workspace = true` -> `promptforge-api-runtime.workspace = true` in `workshop-server` and `workshop-sessions`.
- Identifier sweeps over every `.rs` file (sources, doc tests, `benches/`, `tests/`), in this order:
  1. `shared_promptforge_api` -> `promptforge_api_types` (about 90 files).
  2. `\bpromptforge_api\b` -> `promptforge_api_runtime` with a word boundary so `promptforge_api_types` is untouched: runtime `src/lib.rs` doc examples, `benches/models_loop.rs`, `tests/suite/*`, `workshop-sessions` sources, `workshop-server` tests.
- Runtime re-export: add `pub use promptforge_api_types as types;` to `crates/promptforge-api-runtime/src/lib.rs` (alias form; a flat per-module re-export collides with the runtime's own `capabilities`, `observe`, `tools`, `untrusted`, `cancel` modules). Rewrite the crate-level doc example and `crates/promptforge-api-runtime/README.md` (title, badges, usage) to the one-dependency form using `promptforge_api_runtime::types::observe::NullObserver`.
- `crates/build-xtask/src/product.rs`:
  - `workspace_crates`: record each crate's manifest directory alongside package name and dependency list; walk `crates/` recursively - a directory containing `Cargo.toml` is a crate (no descent), otherwise it is a container and the walk descends one level.
  - `boundary_breach`: add the positional container rule - when a dependency's directory is under `crates/<container>/` and `<container>` has no manifest, the dependent must be under the same container or be that container's named public crate (`crates/promptforge/` -> `promptforge-api-runtime`). Violation text: `<package> depends on <dep>: crates/promptforge is private to its family; only promptforge-api-runtime may depend into it`.
  - Widen the single-entry exception to the set `{promptforge-api-runtime, promptforge-api-types}`; `family()` needs no change since both start with `promptforge-`.
  - Update the doc comment and existing fixture strings to the new names; give `write_crate` a directory-path parameter or add a `write_nested_crate` sibling.
  - New fixture tests on a temp workspace: an outside crate depending on a crate under `crates/promptforge/` is reported with the privacy message; a container crate depending on a container sibling passes; `promptforge-api-runtime` depending into the container passes; an outside crate depending on root-level `promptforge-api-runtime` or `promptforge-api-types` passes.
- `crates/build-xtask/src/tidy.rs` `participating_crates`: use the same recursive walk so the `## Invariants` marker scan still sees every crate.
- Path literals: `.github/actions/hf-model-cache/action.yml` -> `hashFiles('crates/promptforge/tool-picker/build.rs')`; chunk name in `crates/promptforge/lua/src/messages.rs` -> `@crates/promptforge/lua/src/__impl_messages.lua`; chunk name in `crates/promptforge/lua/src/coro.rs` -> `@crates/promptforge-api-runtime/src/lua/__impl_coro.lua` together with the four matching asserts in `crates/promptforge-api-runtime/src/lua-coro-tests.rs`.
- Prose sweeps (`shared-promptforge-api` -> `promptforge-api-types`, `promptforge-api` -> `promptforge-api-runtime`) everywhere outside `vibe/`: runtime `lib.rs` crate doc; the types crate's `AGENTS.md` and `README.md` titles; `AGENTS.md` in `web-search`, `webfetch`, `model-client`; `README.md` in `model-client`, `web`, `webfetch`, `web-search`; the runtime's `AGENTS.md` rewritten to describe the container instead of listing internal crates; `crates/workshop-server/README.md`; `crates/workshop-sessions/README.md`; `tools/document.md` line 123 (line 132's `promptforge-agent` stays as is).
- Root `AGENTS.md`: line 27 names both public crates and states that crates under `crates/promptforge/` are private with `promptforge-api-runtime` the only outside crate permitted to depend into them; line 29 adds a sentence naming PromptForge's public surface; line 56 lists the privacy rule among what `cargo test -p build-xtask` enforces.
- Guide: edit `guide/src/agent/01-agent-programs.md` line 53, then run `cargo run -p build-user-guide` to regenerate `guide/promptforge-agent-guide.md` and `guide/src/SUMMARY.md`; never hand-edit the assembled file.
- `Cargo.lock`: regenerates with the two new package names on first build; include it in the commit.
- Verification, run once against the final tree, every command exiting zero:
  - `cargo check --workspace --all-targets`
  - `cargo test -p build-xtask`
  - `cargo nextest run --locked -p promptforge-api-runtime -p promptforge-api-types -p promptforge-lua -p promptforge-parser -p workshop-sessions -p workshop-gateway -p workshop-protocol`
  - `cargo nextest run --locked -p workshop-server --features headless`
  - `cargo test --doc -p promptforge-api-runtime -p promptforge-api-types`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo fmt --all --check`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `mdbook build guide`; `cargo deny check` when installed
  - `rg 'shared-promptforge-api|shared_promptforge_api|\bpromptforge_api\b' --glob '!vibe/**' --glob '!target/**'` returns nothing
  - `ls crates/` shows `promptforge-api-runtime`, `promptforge-api-types`, and `promptforge/` and no other `promptforge-*` entry; `crates/promptforge/` contains exactly nine crate directories
- Commit: exactly one commit containing all of the above. The message records that the user approved the new topology check (the container privacy rule) during planning, satisfying the repository policy exception in `AGENTS.md`. `git status` is clean afterward.

</step-1>

</execution-plan>
