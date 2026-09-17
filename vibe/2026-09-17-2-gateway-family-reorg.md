---
name: Gateway family reorganization
overview: Apply the promptforge container shape to the gateway family: rename shared-gateway-api and shared-gateway-discovery to gateway-api and gateway-api-discovery at the root, move all twelve gateway crates plus shared-cloud-providers into a fully private crates/gateway/ container with a nested STT subsystem, and extend the existing boundary checks. One commit.
todos:
  - id: single-commit
    content: "One commit: harness updates, moves, renames, sweeps, path fixes, docs, verification"
    status: pending
isProject: false
---

# Gateway Family Reorganization

<product-contract>

## Product Requirements

The gateway family's public and private crates are distinguished only by name-prefix convention, and two of its public API crates are misfiled under `shared-*`. This plan makes the split a property of the directory tree: two public crates at the root of `crates/`, a fully private `crates/gateway/` container holding the twelve family crates plus the cloud-providers tool, and a nested `crates/gateway/stt/` subsystem whose only externally visible crate is `gateway-stt`. The change lands as exactly one commit.

- Problem and users: Maintainers and coding agents cannot tell from the tree which gateway crates are public API and which are machinery; `shared-gateway-api` and `shared-gateway-discovery` are consumed as the gateway's public surface but are named and placed as cross-product substrate. The STT stack (`gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-whisper-ffi`) is a four-crate subsystem with one entry point, flat among siblings it should not be exposed to.
- Goals:
  - The gateway family's public surface is two root-level crates: `gateway-api` (the provider model sheet schema, pure vocabulary) and `gateway-api-discovery` (the discovery file, launch lock, stale detection, health probe).
  - `crates/gateway/` holds the binary (`app/`, package `gateway`), the seven capability crates, and the cloud-providers tool, fully private: no crate outside the container may depend on anything inside it.
  - `crates/gateway/stt/` holds the STT subsystem; only `gateway-stt` (at `stt/api/`) may be named outside it.
  - `cargo test -p build-xtask` and `cargo test -p gateway-stt --test it architecture` enforce all of the above.
- Non-goals: moving or renaming workshop crates; flattening `src/` (declined); the gateway app module decomposition (a separate plan, runs after); consolidating the two boundary harnesses into one; renaming `backend-whisper` to `whisper-backend` (evaluated, kept).
- Success criteria: every gate in the Testing Plan exits zero; the new fixture tests report the forbidden shapes and accept the legal ones; `rg 'shared-gateway|shared_gateway' --glob '!vibe/**' --glob '!target/**'` returns nothing; `ls crates/` shows `gateway-api`, `gateway-api-discovery`, and `gateway/` and no other `gateway-*` entry.
- Constraints:
  - One commit. No intermediate commits, no provisional commits left in history.
  - Package names of moved crates do not change, so no `use` statements, `cargo -p` flags, or nextest `package(...)` filters change. The three renames are `shared-gateway-api` -> `gateway-api`, `shared-gateway-discovery` -> `gateway-api-discovery`, `shared-cloud-providers` -> `gateway-cloud-providers`.
  - Cargo prunes excluded subtrees during member-glob expansion (established in the promptforge reorg), so `members` lists every container crate explicitly and `exclude` lists the containers.
  - Repository policy (`AGENTS.md`, Engineering, "Repository policy binds plans") requires explicit user approval for new structural enforcement. The nested-scope generalization of the container rule is new enforcement logic and was approved when the user added the STT nesting. The commit message records this.
- Open questions: None.

## Functional Specification

Three actors interact with the result: an outside crate author (the workshop family), an inside crate author working on the gateway, and the two structural harnesses that check every manifest. Observable behavior is which dependency edges compile clean and which the harnesses report.

- Actors and workflows:
  - Outside author (workshop crates): attaches to a running gateway through `gateway-api-discovery` and reads the model sheet through `gateway-api`. Nothing else in the family is nameable; the matrix's Workshop-to-Gateway arm exempts exactly this pair.
  - Inside author: crates under `crates/gateway/` see their siblings; crates under `crates/gateway/stt/` see only the subsystem, and the rest of the family sees only `gateway-stt`.
  - Harnesses: `build-xtask` (`crates/build-xtask/src/product.rs`) walks `crates/` recursively and checks every manifest's dependency tables; the gateway-stt integration test (`crates/gateway-stt/tests/it/architecture.rs`, moving with its crate) checks the product matrix from cargo metadata.
- Inputs and outputs: input is every `Cargo.toml` under `crates/` (normal, dev, build, and target-specific tables); output is a list of violation strings, empty on success.
- States and validation: a directory under `crates/` with no `Cargo.toml` is a container; a directory with one is a crate and the walker does not descend. A crate is visible to crates under its parent directory, except designated public members, which are visible one level higher: `gateway-stt` is the public member of `crates/gateway/stt/`. Named outside exceptions remain per container: `promptforge-api-runtime` into `crates/promptforge/`; `crates/gateway/` and `crates/gateway/stt/` have none.
- Errors and recovery: a violation reads `<package> depends on <dep>: crates/gateway is private to its family; no outside crate may depend into it` (or the `gateway/stt` equivalent naming `gateway-stt`). Recovery is depending on the public member or moving the needed type into a public crate.
- Security and privacy behavior: None beyond the dependency boundary; no runtime behavior changes.
- Acceptance criteria:
  - `crates/gateway/` contains `app/`, `cloud-providers/`, `stt/`, and the seven capability directories, and no `Cargo.toml` of its own; `crates/gateway/stt/` contains `api/`, `engine/`, `backend-whisper/`, `whisper-ffi/` and no `Cargo.toml`.
  - The app's test support (`src/test_support.rs`, `tests/it/realtime_stt.rs`) reaches engine fixtures through `gateway_stt::test_fixtures::...`; `gateway/Cargo.toml` no longer declares a `gateway-stt-engine` dev-dependency.
  - The harness fixtures: outside-into-`gateway/` fails; sibling edges pass; workshop-to-`gateway-api-discovery` passes; workshop-into-`gateway/` fails; family-into-`stt/engine` fails; family-to-`gateway-stt` passes; `backend-whisper`-to-`engine` passes.

</product-contract>
<implementation-contract>

## Technical Design

The tree becomes the boundary and the harnesses read the tree. Two renames turn the misfiling shared crates into the gateway's public pair; thirteen moves put the machinery under a manifestless container with one nested subsystem; one re-export extension reroutes the app's test seam through the subsystem's public face. No `.rs` module file moves relative to its own crate.

- Architecture:
  - Target tree: `crates/gateway-api/`, `crates/gateway-api-discovery/`, and `crates/gateway/` containing `app/` (the binary, package `gateway`), `cloud-providers/` (package `gateway-cloud-providers`), `config/`, `config-ui/`, `local/`, `logging/`, `protocol/`, `routing/`, `web-search/`, and `stt/` containing `api/` (package `gateway-stt`), `engine/`, `backend-whisper/`, `whisper-ffi/`.
  - Dependency direction is unchanged: the spine is `config <- protocol <- {routing, web-search} <- local`, with `stt/api` above `config` and `local`, and `app` composing everything. The public pair has no edges into the family: `gateway-api` depends only on `serde` and `time`; `gateway-api-discovery` on no workspace crate.
  - Forced moves: once `shared-gateway-api` becomes `gateway-api`, the family matrix forbids `shared-cloud-providers` (Shared) from depending on it, so it moves and renames. The same rename makes workshop's `gateway-api-discovery` edges violate the Workshop-to-Gateway arm, so that arm gains the public-pair exemption.
- Modules and interfaces:
  - `crates/build-xtask/src/product.rs`: add `PUBLIC_GATEWAY: [&str; 2] = ["gateway-api", "gateway-api-discovery"]` and exempt it in the `(Family::Workshop, Family::Gateway)` arm of `boundary_breach`. Generalize the container rule to nested scopes: `container_of` returns the deepest container (`crates/gateway/stt/engine` is in `gateway/stt`); per-container tables record the named outside exception (`crates/promptforge/` keeps `promptforge-api-runtime`; the gateway containers have none) and the public member (`crates/gateway/stt/` has `gateway-stt`). New fixtures in `product-tests.rs` cover every acceptance-criteria shape. The rule-list doc comment (lines 6-19) gains the gateway container and the nested subsystem.
  - `crates/gateway/stt/api/tests/it/architecture.rs` (today `crates/gateway-stt/tests/it/architecture.rs`): `workspace_root()` stops counting parents and walks ancestors until a `Cargo.toml` containing `[workspace]` is found, so no future move breaks it; the "Workshop cannot depend on Gateway" rule gains the same public-pair exemption; the adversarial fixture keeps its 7-violation count because its targets are not named `gateway-api*`.
  - `crates/gateway/stt/api/src/test_fixtures.rs` (today `crates/gateway-stt/src/test_fixtures.rs`, lines 25-27) already re-exports `DecodeMode`, `ScriptedDecoder`, `ScriptedModelFactory`; extend it with `ModelFactory`, `EnginePolicy`, `Decoder`, `TranscribeError`, and the `native` fixture module, then repoint `gateway/src/test_support.rs` (lines 97-146) and `gateway/tests/it/realtime_stt.rs` (line 13) to `gateway_stt::test_fixtures::...` and drop the `gateway-stt-engine` dev-dependency from `crates/gateway/app/Cargo.toml` (line 164). The repository rule binds dev-dependencies, so the edge cannot be exempted.
- File and public API changes:
  - Moves: the twelve gateway crates to `crates/gateway/<short>/` (`gateway` -> `app`, the STT four into `stt/`, the rest dropping the `gateway-` prefix); `shared-cloud-providers` to `crates/gateway/cloud-providers` with package rename and its explicit `[lib] name` and internal `shared_cloud_providers::` paths updated.
  - Renames: `crates/shared-gateway-api` -> `crates/gateway-api`, `crates/shared-gateway-discovery` -> `crates/gateway-api-discovery`, packages renamed to match.
  - Root `Cargo.toml`: `members` adds the fourteen container paths explicitly; `exclude` adds `"crates/gateway"` and `"crates/gateway/stt"`; `default-members` (line 15) becomes `crates/gateway/app`; `[workspace.dependencies]` repoints the twelve gateway paths (lines 30-65) and renames the two shared keys and the cloud-providers key.
  - Member manifests: `shared-gateway-api.workspace = true` -> `gateway-api...` in app, config, protocol; `shared-gateway-discovery...` -> `gateway-api-discovery...` in app, workshop, workshop-server, workshop-gateway (normal and dev tables).
  - Identifier sweeps, ordered: `shared_gateway_discovery` -> `gateway_api_discovery` first, then `shared_gateway_api` -> `gateway_api` (about 50 files); kebab prose sweep `shared-gateway-discovery` -> `gateway-api-discovery`, then `shared-gateway-api` -> `gateway-api`, excluding `vibe/`.
  - Path literals: `crates/gateway/app/build.rs` line 23 `ICON` becomes `../../workshop/icons/icon.ico`; `crates/build-ui/src/lib.rs` line 125 replaces `ui_dir/../../shared-ui` with an upward search for `crates/shared-ui` (still correct for `crates/workshop-server`); `crates/gateway/config-ui/ui/package.json` line 21 becomes `"shared-ui": "file:../../../shared-ui"` and `npm ci` regenerates the lockfile; `crates/gateway/config-ui/ui/build.mjs` line 20 gains one more `..`.
  - Repository config: `.gitignore` lines 13, 18, 20 and `.gitattributes` lines 11-15 gain the `gateway/` prefix; `dist-workspace.toml` line 4 becomes `cargo:crates/gateway/app` (do not run `cargo dist generate`; the generated release workflow is hand-edited and CI validates the plan on PRs); `.github/workflows/` npm paths `crates/gateway-config-ui/ui` -> `crates/gateway/config-ui/ui` (ci.yml nine lines, nightly.yml, release-workshop.yml, promptforge-gateway-v-release.yml lines 146-148, llama-cuda-blackwell.yml lines 106 and 110 plus its line-7 comment, dist-ci/build-setup.yml lines 14 and 16); `cache-dependency-path: crates/*/ui/package-lock.json` entries gain `crates/gateway/*/ui/package-lock.json`; `whisper-lib.yml` line 17 trigger becomes `crates/gateway/stt/whisper-ffi/**`; `tools/document.md` line 114 gateway targets list.
  - Docs: root `AGENTS.md` line 25 (gateway rule names the public pair and the private container), line 29 (the `shared-*` sentence loses the two gateway crates), line 56 (enforcement list gains the gateway container, the nested subsystem, and the second harness); titles and rule text in the renamed crates' `AGENTS.md`/`README.md` and a sweep of the 19 gateway-family doc files; `guide/src/workshop/01-application.md` line 1 then `cargo run -p build-user-guide` to regenerate; `workshop-server/README.md` and `workshop/AGENTS.md` name `shared-gateway-discovery`; stale doc comments at `gateway/app/src/boot.rs` line 530, `gateway/app/tests/it/icon.rs` line 2, `gateway/local/src/artifacts/assets.rs` line 207, `gateway/config-ui/src/assets.rs` line 37.
  - `Cargo.lock` regenerates with the three new package names and is part of the commit.
- Data, persistence, failure, security, and privacy constraints: nothing persisted or on the wire changes; the only new failure mode is a harness test failure with the violation text above; the `gateway-api-discovery` crate keeps its hand-mirrored lint table (`unsafe_code = "deny"` for the `src/sys/` FFI shims) untouched.

</implementation-contract>
<verification-contract>

## Testing Plan

Verification is the repository's existing gate set plus the new fixture tests, run once against the final tree because the change is one commit.

- Unit: the new `product-tests.rs` fixtures covering every acceptance-criteria shape (outside-into-container reported, siblings pass, public-pair exemption passes, family-into-`stt/engine` reported, family-to-`gateway-stt` passes, subsystem-internal edges pass); the existing single-entry and promptforge-container fixtures keep passing.
- Integration and end-to-end: `cargo check --workspace --all-targets`; `cargo check -p gateway --no-default-features` (the pre-push hook's headless gate); `cargo nextest run --locked -p gateway -p gateway-api -p gateway-api-discovery -p gateway-cloud-providers -p gateway-config -p gateway-config-ui -p gateway-local -p gateway-protocol -p gateway-routing -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-web-search -p workshop -p workshop-server -p workshop-gateway`; `cargo nextest run --locked -p workshop-server --features headless`; `cargo test -p gateway-stt --test it architecture`; `npm ci && npm run typecheck && npm test` in `crates/gateway/config-ui/ui`.
- Regression, security, and performance: `cargo test -p build-xtask`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and the workshop variant; `cargo fmt --all --check`; `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; `mdbook build guide`; `cargo deny check` when installed. No performance change expected; moves and renames generate no code.
- Exit criteria: all commands above exit zero; `rg 'shared-gateway|shared_gateway' --glob '!vibe/**' --glob '!target/**'` returns nothing; `git status` clean after the single commit.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Public pair named `gateway-api` and `gateway-api-discovery` at the root; everything else under `crates/gateway/`. Rationale: the user's design - "we make crates/gateway-api-* the public APIs of Gateway, we put all the other gateway crates into crates/gateway/* and those are private".
  - The binary crate's directory is `app/`. User selected `app` over `gateway/gateway/` and `bin/` when asked.
  - The STT subsystem nests as `crates/gateway/stt/{api,engine,backend-whisper,whisper-ffi}/` with only `gateway-stt` visible outside. User selected "Add it" when asked; the subsystem has the shape the rule needs (four crates, one entry point, a chain inside).
  - Package names of moved crates unchanged. Rationale: no `use` sites, `-p` flags, or nextest filters change; precedent from the promptforge reorg.
  - The app's `gateway-stt-engine` dev-dependency reroutes through `gateway-stt`'s extended `test_fixtures` re-export rather than an exemption. Rationale: the repository rule binds dev-dependencies, and the seam already exists; improving an existing facility beats new machinery.
  - `architecture.rs` finds the workspace root by walking ancestors for `[workspace]` instead of counting parents. Rationale: depth counting has now broken twice and breaks at every future move.
  - One commit, plan copy in `vibe/`, strict one-commit run variant. User's words: "run ... as a SINGLE commit (NO EXCEPTIONS) and copy the plan into vibe/ following the format of the files in there".
- Rejected alternatives:
  - `crates/gateway/impl/` for the whole family with a nobody-sees-inside rule. Reason: the container already is the implementation tier; the public pair has no edges into the family, so there is no second audience to separate. Revisit if the family grows a public behavior crate.
  - Nesting `config/` and `config-ui/` together. Reason: `config-ui` does not depend on `config`; the grouping would be by name, not by graph. Revisit if the edge appears.
  - Renaming `backend-whisper` to `whisper-backend`. Reason: evaluated and kept as is. Revisit never unless the package is renamed.
  - A crate or file per route in the app. Reason: wrong grain; routes share `AppState`, auth, and relay machinery, and the repo's grain is feature modules. The feature-module split is its own plan.
  - Flattening `src/`. Reason: user declined ("leave src/").
  - Running `cargo dist generate` after the `dist-workspace.toml` path change. Reason: the generated workflow carries hand edits its header warns about; CI's `pr-run-mode = "plan"` validates instead.
- Assumptions, risks, and notes:
  - The `dist-workspace.toml` member path is exercised only on release tags; CI's plan check is the mitigation. Confidence medium.
  - Platform-specific paths (the icon relative path, the whisper trigger) are verified by the macOS and Linux CI legs, not locally.
  - Two harnesses now enforce overlapping matrices (`product.rs` and `architecture.rs`); keeping both in sync is a standing tax. Consolidation is deferred, not forgotten.
  - Explicit `members` enumeration means a future fifteenth gateway crate fails loudly ("believes it's in a workspace") until listed; accepted, same as promptforge.
  - `.config/nextest.toml` heavy-group filters match by package name; unaffected.
  - `vibe/` history is not rewritten.

### Deferred and Out of Scope

- Deferred: the gateway app module decomposition (lib.rs feature split); its own plan, runs immediately after this commit.
- Deferred: consolidating `architecture.rs` into `build-xtask`; revisit when the matrices next diverge.
- Deferred: applying the container shape to the workshop family; revisit once the gateway shape has been lived with.
- Out of scope: moving or renaming any workshop crate; flattening `src/`; SPA changes beyond the path fixes listed; `vibe/` history.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build: `cargo build` (default member is the gateway, which builds on a fresh clone with no CUDA or Tauri system packages); the desktop app is explicit: `cargo build -p workshop`. CI uses `cargo build --locked -p gateway` and `cargo build --locked -p gateway --no-default-features`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <name-filter>`; gateway integration tests also run via `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <name>`.
- Component test command pattern: `cargo nextest run --locked -p <crate>` (add `--all-features` where the crate gates features); structural and boundary harness: `cargo test -p build-xtask`; product dependency boundary check: `cargo test -p gateway-stt --test it architecture`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` plus `cargo nextest run --locked -p workshop-server --features headless` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` (workshop partition: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`). Supply chain: `cargo deny check` and `cargo audit`.
- Formatter check: `cargo fmt --all --check`.
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`.
- Test placement and naming: unit tests live inline in `#[cfg(test)]` modules, with large ones split into kebab sibling files named `<module>-tests.rs` wired by `#[path = "..."]` (e.g. `boot_load-tests.rs`); integration tests live in `tests/it/` behind a single `main.rs` harness so the target is named `it`; test functions are long descriptive snake_case sentences (e.g. `a_process_lifetime_lease_recovers_after_its_owner_is_terminated`); Node tool scripts carry sibling `.test.mjs` files.
- Directory map: `crates/` holds all workspace members in families - `gateway-*` (gateway service), `promptforge/` (manifestless container of nine private executor crates) plus `promptforge-api-runtime` and `promptforge-api-types` (the family's only public door), `workshop-*` (desktop app and server), `shared-*` (cross-product API surface; `shared-ui` is TypeScript+CSS, not a Rust crate), `build-*` (build tooling including the `build-xtask` structural harness); `guide/` is the mdbook user guide plus assembled product guides; `prompts/` holds prompt pipelines; `tools/` holds Node utility scripts; `vibe/` holds plans and `archdoc.md`; `.github/workflows/ci.yml` is the CI definition; `target/` and `target-msrv/` are build outputs.
- Component boundaries: three products - PromptForge (executor), Gateway (inference service), Workshop (Tauri desktop). Dependency rules: workshop-* never depends on gateway-*; gateway-* never depends on promptforge or workshop crates; promptforge-* never depends on gateway or workshop crates; outside crates may depend only on promptforge-api-runtime and promptforge-api-types, never the private crates under `crates/promptforge/`; shared-* depends on no product crates; the `workshop` shell depends on `workshop-server-api` and never on `workshop-server`. Dependencies flow shell -> features -> services -> vocabulary, enforced by `cargo test -p build-xtask`.
- Conventions summary: edition 2024 workspace with workspace-inherited lints (unsafe_code forbidden, clippy all denied, pedantic warned, unwrap/expect denied); no file exceeds 500 lines (split first, then edit); source directories are flat by default - subdirectories need at least three files, otherwise kebab sibling files with explicit `#[path]` attributes; every workshop-* lib.rs opens with a `//!` doc carrying a `## Invariants` marker; behavior changes ship with tests in the same change; comments explain non-obvious constraints and cite upstream issue URLs for workarounds; error messages are written for model consumption; CSS lives beside its TypeScript using `--ws-*` tokens; the SPA never touches localStorage, persisting through the `ui-storage` adapter instead.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: gateway family reorganization [completed]

- Component: `none`

Apply the whole reorganization as one commit, in this order:

1. Harness updates: in `crates/build-xtask/src/product.rs` add `PUBLIC_GATEWAY: [&str; 2] = ["gateway-api", "gateway-api-discovery"]`, exempt it in the `(Family::Workshop, Family::Gateway)` arm of `boundary_breach`, generalize `container_of` to return the deepest container, add per-container public-member and named-exception tables (`crates/promptforge/` keeps `promptforge-api-runtime`; `crates/gateway/stt/` has public member `gateway-stt`; gateway containers have no exceptions), update the rule-list doc comment, and add `product-tests.rs` fixtures for every acceptance-criteria shape. In `crates/gateway-stt/tests/it/architecture.rs` change `workspace_root()` to walk ancestors for a `Cargo.toml` containing `[workspace]` and add the public-pair exemption to the Workshop-to-Gateway rule.
2. Moves and renames: `crates/shared-gateway-api` -> `crates/gateway-api`, `crates/shared-gateway-discovery` -> `crates/gateway-api-discovery` (packages renamed to match); the twelve gateway crates into `crates/gateway/<short>/` (`gateway` -> `app`, the STT four into `stt/{api,engine,backend-whisper,whisper-ffi}`, the rest dropping the `gateway-` prefix); `shared-cloud-providers` -> `crates/gateway/cloud-providers` with package rename to `gateway-cloud-providers` and its explicit `[lib] name` and internal `shared_cloud_providers::` paths updated.
3. Test seam: extend `crates/gateway/stt/api/src/test_fixtures.rs` with `ModelFactory`, `EnginePolicy`, `Decoder`, `TranscribeError`, and the `native` fixture module; repoint `crates/gateway/app/src/test_support.rs` and `crates/gateway/app/tests/it/realtime_stt.rs` to `gateway_stt::test_fixtures::...`; drop the `gateway-stt-engine` dev-dependency from `crates/gateway/app/Cargo.toml`.
4. Manifests: root `Cargo.toml` gains the fourteen container paths in `members`, `"crates/gateway"` and `"crates/gateway/stt"` in `exclude`, `default-members` becomes `crates/gateway/app`, and `[workspace.dependencies]` repoints the twelve gateway paths and renames the two shared keys and the cloud-providers key; member manifests switch `shared-gateway-api.workspace = true` -> `gateway-api...` (app, config, protocol) and `shared-gateway-discovery...` -> `gateway-api-discovery...` (app, workshop, workshop-server, workshop-gateway, normal and dev tables).
5. Sweeps, ordered: `shared_gateway_discovery` -> `gateway_api_discovery`, then `shared_gateway_api` -> `gateway_api` (about 50 files); then kebab prose `shared-gateway-discovery` -> `gateway-api-discovery`, then `shared-gateway-api` -> `gateway-api`, excluding `vibe/`.
6. Path literals and repo config: `crates/gateway/app/build.rs` `ICON` path, `crates/build-ui/src/lib.rs` upward search for `crates/shared-ui`, `crates/gateway/config-ui/ui/package.json` (`"shared-ui": "file:../../../shared-ui"`, then `npm ci` to regenerate the lockfile) and `build.mjs`, `.gitignore`, `.gitattributes`, `dist-workspace.toml` (`cargo:crates/gateway/app`, do not run `cargo dist generate`), the `.github/workflows/` npm and cache paths plus `whisper-lib.yml` trigger and `llama-cuda-blackwell.yml` comment, and `tools/document.md`.
7. Docs: root `AGENTS.md` lines 25, 29, 56; renamed crates' `AGENTS.md`/`README.md`; the 19 gateway-family doc files; `guide/src/workshop/01-application.md` then `cargo run -p build-user-guide`; `workshop-server/README.md` and `workshop/AGENTS.md`; stale doc comments in `gateway/app/src/boot.rs`, `gateway/app/tests/it/icon.rs`, `gateway/local/src/artifacts/assets.rs`, `gateway/config-ui/src/assets.rs`.
8. Regenerate `Cargo.lock` (three new package names), run the full Testing Plan once against the final tree, confirm `rg 'shared-gateway|shared_gateway' --glob '!vibe/**' --glob '!target/**'` returns nothing, and land everything as exactly one commit whose message records the approved policy exception for the nested-scope enforcement.

</step-1>

</execution-plan>
