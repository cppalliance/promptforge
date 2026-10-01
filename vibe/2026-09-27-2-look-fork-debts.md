---
name: "Debt removal: look fork"
overview: "Remove the two debts that PR #81 (f4f92899..6a0ad504) added: build-ui's unbounded esbuild and workspace searches, and look's theming docs and inlined hover color that contradict look's own stylesheets. Two independent work items, no hard-to-reverse changes."
todos:
  - id: w1-install-root
    content: "W1 (D1-1, D1-3): add install_root in crates/build-ui/src/lib.rs, bound find_esbuild and workspace_members at it, add the three failing-first tests and update the existing ones in lib-tests.rs, verify with build-ui tests and both UI cargo builds"
    status: pending
  - id: w2-look-theming
    content: "W2 (D1-2): add --accent-bright to look/tokens.css and use it in look/controls.css line 48; reword look/AGENTS.md line 7, AGENTS.md line 89, workshop-spa.mdc line 20; verify with the git grep checks, workspace npm suite, and a visual hover check"
    status: pending
isProject: false
---

# Remove the debt added by the shared-ui fork

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `c:\Users\Vinnie\cursor\promptforge2`. Every path below is relative to its root.
  - Target `f4f92899..6a0ad504`: `aeb31b78`, `7cabddf7`, `aa137b5e`, `dbd53ec5`, `e66d08fe`, `6a0ad504`. These are the six commits of [cppalliance/promptforge#81](https://github.com/cppalliance/promptforge/pull/81) on branch `vibe2`.
  - The baseline `f4f92899` is `upstream/master`. The disposition ref is `6a0ad504`, and the design record is `vibe/2026-09-27-1-fork-shared-ui-look.md`.
- Cleanup goals:
  - `build-ui` never runs an esbuild from outside the UI's own install root, so a missing install fails with the setup message again (D1-1).
  - `look`'s docs name the file that actually themes `look`'s components, and the target's one new raw color in component CSS goes back behind a palette token (D1-2).
- Non-goals:
  - No change to Gateway code or `crates/shared-ui`.
  - No visible change to the Workshop.
  - No new structural check.
  - No fix of the pre-existing token-vocabulary drift listed under Deferred.
- Success criteria:
  - The new `build-ui` regression tests fail before the change and pass after it. Every existing `build-ui` test passes, updated as the Testing Plan lists.
  - `cargo build -p workshop-server` and `cargo build -p gateway-config-ui` behave as before when the installs are present.
  - No "themes the family by editing" sentence remains outside the planning records under `vibe/`, and `look/controls.css` contains no `#ffffff`.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1: unbounded esbuild search (introduced in `aeb31b78`, still present at `6a0ad504`).
    - Evidence: `find_esbuild` (`crates/build-ui/src/lib.rs` line 236) walks `normalize(ui_dir).ancestors()` with no bound, past the install root and the repository, up to the drive root.
    - Contracts it contradicts: the module doc (lines 13-15: "one `npm ci` per install root ... there is no fallback"), the no-`npx` rationale in the `bundle` doc (line 266), and the fork plan's "a missing install fails the Cargo build with a message naming the directory to run `npm ci` in".
    - Impact when the UI's install is missing and an ancestor directory has `node_modules/.bin/esbuild`:
      - Normally the build fails with esbuild "Could not resolve" errors instead of the setup message.
      - With dev dependencies omitted (`--omit=dev` or `NODE_ENV=production`), the build succeeds silently with an unpinned esbuild. That is the hazard `66f8f95f` removed when it dropped the `npx esbuild` fallback that `d28579e0` had added.
      - Neither case is reachable on the operator's machine today.
    - Reversal cost: low. `build-ui` is `publish = false`, and no signature changes.
    - Target state: both upward searches stop at the install root. `workspace_members` (line 184) has the same unbounded search (D1-3). D1-3 is rejected on its own, but it shares the cause and takes the same bound.
  - D1-2: theming claims that contradict `look`'s stylesheets (introduced in `dbd53ec5`, narrowed by the challenge).
    - Evidence, the theming sentences:
      - `crates/workshop/look/AGENTS.md` line 7 says "A designer themes the family by editing `semantic.css`."
      - `AGENTS.md` line 89 and `.cursor/rules/workshop-spa.mdc` line 20 say "A designer themes the family by editing look/semantic.css."
      - No `look` component sheet reads a `--ws-*` token, and `look/semantic.css` defines no color. Following the rule changes nothing in `look`.
    - Evidence, the raw color: the same commit inlined `color-mix(in srgb, var(--accent) 88%, #ffffff)` at `look/controls.css` line 48. The `look/tokens.css` header (lines 6-7), which that commit rewrote, still says no component stylesheet hardcodes a color.
    - Impact: agent instructions send theming work to the wrong file, and changing the button hover color takes a component edit.
    - Reversal cost: low.
    - Target state: the docs name `tokens.css` as the theming surface for colors and for `look`'s components, and the hover mix lives in a palette token.
- Cheap fixes: none.
- Exposed pre-existing debt: none. Unrelated pre-existing items are listed under Deferred.
- Rejected candidates, 21 in total:
  - Residual-but-acceptable, 5: D1-6 (`crates/shared-ui` watch kept for the Gateway, and its stale comment), D1-7 (orphaned `shared-ui` dropdown, progress and shimmer), D1-9 (`build-ui` `pub` surface), D1-10 (`look` exports map), D1-21 (deferred component-token blocks). Each is either reserved by the fork plan or has no consequence.
  - Weak/speculative, 9: D1-3 (folded into the D1-1 fix), D1-4, D1-5, D1-14, D1-15, D1-16, D1-17, D1-18, C-1. None has a consequence reachable with the current layout or callers.
  - False, 6: D1-8, D1-11, D1-13, D1-19, D1-20, D1-22. The code disproves each claim: no deleted alias is still read, lockfile versions are unchanged, and the root `eol` rule covers the moved files.
  - Unrelated pre-existing, 1: D1-12 (legacy `--space-*` aliases).

</product-contract>
<implementation-contract>

## Technical Design

- Install-root bound in [crates/build-ui/src/lib.rs](c:\Users\Vinnie\cursor\promptforge2\crates\build-ui\src\lib.rs) (D1-1, D1-3):
  - Add a private helper `install_root(ui_dir: &Path) -> PathBuf`. It returns the directory holding `nearest_lockfile(ui_dir)`, or `normalize(ui_dir)` when there is no lockfile.
  - `find_esbuild` checks `node_modules/.bin/esbuild` (`esbuild.cmd` on Windows) in `normalize(ui_dir)` and each ancestor up to and including `install_root(ui_dir)`, never above it. The Gateway config UI then searches only its own `ui/`, and the Workshop UI searches `ui/` and `crates/workshop/`.
  - `workspace_members` reads `package.json` only in `normalize(ui_dir)` and its ancestors up to and including `install_root(ui_dir)`. The Workshop still finds `crates/workshop/package.json`. The Gateway reads only its own manifest and gets `None`, so a `package.json` above the checkout is never read.
  - The `find_esbuild` doc comment says where the search stops. The module doc's "there is no fallback" is true again and stays as written.
  - No signature changes, and neither the watch list nor the text of the missing-install error changes. `esbuild_command` (line 321) may take its error directory from `install_root`.
- Palette token for the hover mix (D1-2):
  - [crates/workshop/look/tokens.css](c:\Users\Vinnie\cursor\promptforge2\crates\workshop\look\tokens.css): in the "Accent and semantics" block, after `--accent-dim` (line 95), add `--accent-bright: color-mix(in srgb, var(--accent) 88%, #ffffff);`. The name must not be one of the 13 deleted Gateway alias names, so `--accent-hover` is ruled out.
  - [crates/workshop/look/controls.css](c:\Users\Vinnie\cursor\promptforge2\crates\workshop\look\controls.css) line 48: `.button-primary:hover` uses `background: var(--accent-bright);`. The computed color is the same.
- Theming sentences (D1-2):
  - [crates/workshop/look/AGENTS.md](c:\Users\Vinnie\cursor\promptforge2\crates\workshop\look\AGENTS.md) line 7: replace "A designer themes the family by editing `semantic.css`." with a sentence saying two things. Colors, and every `look` component, are themed in `tokens.css`. `sizes.css` and `semantic.css` hold the size scale and the semantic aliases that Workshop UI component CSS reads as `--ws-*` tokens.
  - `AGENTS.md` line 89 and `.cursor/rules/workshop-spa.mdc` line 20, kept worded the same:
    - The opening rule applies to Workshop UI component CSS.
    - Replace "A designer themes the family by editing look/semantic.css." with a sentence saying that sizes and elevation are themed in `look/semantic.css`, and colors in `look/tokens.css`, which `look`'s own component sheets read directly.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused tests for D1-1 and D1-3, in `crates/build-ui/src/lib-tests.rs`:
  - New `an_install_above_the_install_root_is_not_used`: create `outer/node_modules/.bin/esbuild` (with `install_esbuild`), `outer/root/package-lock.json`, and `outer/root/ui/`. `find_esbuild(outer/root/ui)` must be `None`. It fails before the change.
  - New `a_workspace_root_above_the_install_root_is_not_adopted`: create `outer/package.json` with a `workspaces` array, plus `outer/root/ui/package-lock.json` and `outer/root/ui/package.json` without `workspaces`. `workspace_members(outer/root/ui)` must be `Ok(None)`. It fails before the change.
  - New `a_malformed_package_json_above_the_install_root_is_not_read`: the same layout, with a malformed `outer/package.json`. `workspace_members` must return `Ok(None)`. It fails before the change.
  - `an_ancestor_install_is_found` also writes `temp/package-lock.json`, the layout npm produces for a workspace.
  - `no_install_in_the_tree_finds_nothing_in_it` asserts `find_esbuild(&ui).is_none()` outright and drops the home-level comment.
  - Some existing tests expect `workspace_members` or `watched_paths` to find a root above `ui_dir`. Each one writes `package-lock.json` at that root. These are the tests at lines 80-183 (`the_other_listed_members_are_returned`, `a_package_json_without_workspaces_is_not_the_root`, `a_malformed_package_json_is_an_error_naming_it`, `the_ui_dir_is_left_out_when_reached_through_a_parent_hop`, `watch_covers_the_workspace_root_and_the_other_members`), plus any other test that fails for the same reason.
  - Commands: `cargo nextest run --locked -p build-ui`, `cargo test --doc -p build-ui`, `cargo clippy -p build-ui --all-targets -- -D warnings`, `cargo fmt --all --check`.
- Integration for D1-1:
  - `cargo build -p workshop-server` and `cargo build -p gateway-config-ui` succeed with the installs present.
  - The differential integration test in `crates/build-ui/tests/it/main.rs` still runs instead of skipping.
  - Operator spot check: rename `crates/gateway/config-ui/ui/node_modules` temporarily. `cargo build -p gateway-config-ui` must fail with the setup message naming `crates/gateway/config-ui/ui`. Restore the folder.
- Checks for D1-2:
  - `git grep -n "themes the family by editing" -- . ":(exclude)vibe/"` returns nothing. Planning records under `vibe/`, including this plan's own copy, quote the old sentence as history.
  - `git grep -n "#ffffff" -- crates/workshop/look/controls.css` returns nothing.
  - `git grep -n -e "--accent-hover" -- crates/workshop` returns nothing.
  - At `crates/workshop`: `npm run typecheck --workspaces --if-present`, `npm run build --workspace ui`, `npm test --workspaces --if-present`. These cover `look/test/boundary.mjs` and `ui/test/docs-claims.mjs`.
  - Visual check: the primary button hover color is unchanged.
- Exit checks:
  - `git status` is clean after build and test.
  - `git diff --stat` shows nothing under `crates/gateway/` or `crates/shared-ui/`.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - Bound the searches at the lockfile's directory:
    - Why: tracked lockfiles mark both real install roots, and that directory is where npm places the esbuild shim for a standalone package and for a workspace member.
    - Consequence: a UI with no lockfile searches only its own directory.
  - Fold D1-3 into the same bound:
    - Why: it has the same cause and uses the same helper.
    - Consequence: nothing above the checkout is read. A malformed, unrelated `package.json` above the repository can no longer fail a Gateway build. That was the falsifier recorded when the fail-on-malformed behavior was added.
  - Use a named palette token, `--accent-bright`:
    - Why: it restores the pre-target state, where the formula lived in a token.
    - Consequence: no visual change.
  - Reword the theming sentences instead of making them true:
    - Why: routing `look`'s sheets through semantic color aliases is a larger change to a pre-existing vocabulary split.
  - Verify D1-2 with one-time `git grep` checks, not a permanent phrase in `ui/test/docs-claims.mjs`:
    - Why: an added phrase would be a source-text ratchet on agent instructions, which needs operator approval.
- User-resolved architecture choices: none.
  - No remediation is hard to reverse.
  - `build-ui` is `publish = false`, and all its callers are in this repository.
  - The token is private to the `@workshop/*` workspace.
- Rejected alternatives:
  - Bounding the searches at the repository root. The Gateway could still pick up a root-level install.
  - Searching only `ui_dir` and the lockfile's directory. It behaves the same for both UIs; the bounded upward loop is the smaller change.
  - Deleting the "no component stylesheet hardcodes a color" claim instead of adding the token. The target should not add to that claim's pre-existing falsehood.
  - Reinstating `--accent-hover`. It is a deleted Gateway alias name, and the fork plan's verification forbids it under `crates/workshop`.
  - Adding "themes the family by editing" to `docs-claims.mjs`. Revisit if the operator approves the exception.
- Assumptions and risks:
  - Both real UIs keep a tracked `package-lock.json` at their install root.
  - A test that relied on an unbounded search surfaces as a failing test, not as a silent change.
  - D1-1's consequences are inferred from reading code, not reproduced.
  - Analysis limits: no repository code was executed, and the 2.3 MB commit log was searched by term rather than read in full.

### Deferred and Out of Scope

- D1-6: the stale comment at `crates/build-ui/src/lib.rs` line 157 ("Both UIs bundle the shared-ui package") and the Workshop's watch of `crates/shared-ui`. Revisit with the Gateway decoupling plan, which removes that search.
- D1-7: the unused `crates/shared-ui` dropdown, progress, and shimmer code. Revisit when `crates/shared-ui` is removed.
- D1-12: the legacy `--space-*` aliases in `look/tokens.css`. Revisit with a token-vocabulary cleanup.
- The pre-existing parts of D1-2. Revisit with a plan that routes component CSS through semantic color aliases.
  - Raw radii, font sizes, and shadow colors in the copied `look` sheets.
  - 17 Workshop UI component files that read palette names directly.
  - `semantic.css` has no color tokens.
  - The SPA rule's example token `--ws-color-bg-surface` does not exist.
- D1-4: glob or object-form `workspaces` entries. Revisit if one appears.
- D1-5: leftover member-level installs. Revisit if a stale `crates/workshop/ui/node_modules` shadows the hoisted install.
- C-1: registry resolution of `"@workshop/look": "*"`. Revisit if `look` ever leaves `workspaces` while the dependency stays.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the default member, the gateway app (package `gateway`, binary `promptforge-gateway`); `cargo build --locked -p gateway --no-default-features` builds the featureless gateway. `cargo workshop` (alias for `cargo run -p build-workshop --`) is the one-command desktop build: it builds the gateway, stages the sidecar, builds `promptforge-workshop` (package `workshop`), and removes the staging. A bare `cargo build --locked -p workshop` needs a gateway already staged at `crates/workshop/desktop/binaries/promptforge-gateway-<target-triple>` (CI stages with `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe` and cleans up with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`). Both web UIs are bundled by esbuild inside Cargo build scripts into `OUT_DIR`, which needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` (both `node_modules` trees are already installed); the SPA bundle alone is `npm run build --workspace ui` run in `crates/workshop`. Installed locally on Windows/PowerShell: stable rustc 1.98, the pinned `nightly-2026-09-05`, cargo-nextest, cargo-deny, cargo-audit, cargo-hakari, Node 24, npm 11.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`, narrowed with `--test it` (`--test suite` for `promptforge` and `harness`) or `--lib`. The Workshop trio (`workshop`, `workshop-server`, `workshop-server-api`) never takes `--all-features`, e.g. `cargo nextest run --locked -p workshop-server <filter>`. Doctests run outside nextest: `cargo test --locked -p <crate> --all-features --doc <filter>`. Gateway process-lease tests need the fixture feature: `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <name>`. TypeScript: `node --test test/<name>.mjs` run in `crates/workshop/ui` or `crates/workshop/look`; `node --test src/<path>.test.mjs` run in `crates/gateway/config-ui/ui`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, then `cargo test --locked -p <crate> --all-features --doc`. Workshop trio: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless` (server-only integration tests without the UI bundle), and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. Every change also runs the structural checks `cargo test -p build-xtask`. TypeScript packages: `npm test --workspace ui` or `npm test --workspace look` run in `crates/workshop`; `npm test` run in `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `cargo nextest run --locked -p workshop-workspace --all-features` (Windows path jail), `cargo nextest run --locked -p workshop-server --features headless`, `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`, `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only` (nightly-only fixtures), `npm test --workspaces --if-present` in `crates/workshop`, and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Clippy replaces `cargo check`; the only sanctioned standalone check is the headless gate `cargo check -p gateway --no-default-features`. Other gates: `cargo +nightly-2026-09-05 xtask api --check` (facade surface against `crates/promptforge/public-api.txt`), `cargo deny check`, `cargo audit`, and the TypeScript typechecks `npm run typecheck --workspaces --if-present` in `crates/workshop` and `npm run typecheck` in `crates/gateway/config-ui/ui`. No ESLint or other TypeScript linter is configured.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`). No TypeScript or CSS formatter is configured.
- Docs command: with `RUSTDOCFLAGS=-D warnings` set (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the facades with default features, `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, then `cargo doc --locked --no-deps -p workshop-server --document-private-items`. User guide: `cargo xtask site --books-only`.
- Test placement and naming conventions:
  - Rust unit tests live in a `#[cfg(test)] mod tests` per source file, either inline or in a sibling `<stem>-tests.rs` wired with `#[path = "<stem>-tests.rs"]` (e.g. `session.rs` beside `session-tests.rs`); a module whose tests reach three or more files moves them to `src/<module>/tests/` (e.g. `engine/src/execute/tests/`).
  - Integration tests compile into one binary per crate, auto-discovered from `tests/it/main.rs` (target `it`) with modules as sibling files and subdirectories; `promptforge` and `harness` use `tests/suite/main.rs` (target `suite`); `build-workshop`, `gateway-cloud-providers`, `gateway-stt-engine`, and `gateway-stt-backend-whisper` keep flat `tests/<name>.rs` files. Helpers go in `tests/common/`, data in `tests/fixtures/`, prompt fixtures in `tests/prompts/`; test-only hooks sit behind `cfg(test)` or a `test-fixtures` feature.
  - Test names are snake_case behavior sentences with no `test_` prefix, e.g. `a_missing_bundle_file_is_not_found`, `rerank_response_rejects_malformed_results`.
  - `unwrap` and `expect` are allowed only in tests (root `clippy.toml`).
  - TypeScript tests use Node's built-in runner with jsdom: `.mjs` files in `crates/workshop/ui/test/` (helpers in `test/helpers/`) and `crates/workshop/look/test/`; the config UI co-locates `src/**/*.test.mjs`; tool scripts pair `tools/<name>.mjs` with `tools/<name>.test.mjs`.
  - Behavior changes ship with tests in the same change; criterion benches live in `benches/` of `promptforge-engine` and `promptforge-lua`.
- Directory map:
  - `crates/` root is the public and shared layer: `promptforge` (engine facade with committed `public-api.txt`), `harness` (Harness facade), `gateway-api-types` and `gateway-api-discovery` (the gateway's public pair), `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS package for the gateway config UI, not a Rust crate), `workspace-hack` (cargo-hakari), and `build-*` tooling (`build-xtask` structural checks, scaffolder, API check, and docs site; `build-workshop` desktop build orchestrator; `build-ui` esbuild helper; `build-user-guide`; `build-llama-cuda`).
  - `crates/promptforge-internal/`: the private engine family: `engine`, `types`, `vfs`, `lua`, `parser`, `store`, `model-client`.
  - `crates/harness-internal/`: the private harness family: `runner`, `sessions`, `models`, `capabilities`, `log`, `web`, `webfetch`, `web-search`.
  - `crates/gateway/`: the private gateway family: `app` (package `gateway`), `cloud-providers` (own `shared-cloud-providers` binary; no crate depends on it), `config`, `config-ui` (Rust server plus `ui/` SPA), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` (`api` is package `gateway-stt`, plus `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/workshop/`: the private Workshop family and npm workspace root (workspaces `ui` and `look`): `desktop` (Tauri app, package `workshop`, binary `promptforge-workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, `ui` (TypeScript SPA), and `look` (`@workshop/look` tokens and components, the Workshop's fork of `shared-ui`).
  - `guide/`: user guide chapters (`guide/src/<set>/`), books, landing, and chrome for the docs site.
  - `prompts/`: sample prompt programs.
  - `tools/`: Node scripts with `.test.mjs` pairs (`stage-gateway-sidecar.mjs`, `gateway-tts-live.mjs`) and Python helpers in `tools/scripts/` for Dokuman facade-doc runs (`survey.py`, `gates.py`, `check_docs.py`, and others).
  - `vibe/`: planning artifacts: `archdoc.md`, dated `YYYY-MM-DD-N-<slug>.md` plans, monthly archive folders, and a git-ignored `scratch/`.
  - `.github/`: workflows (`ci.yml` with the `ci-green` aggregate gate, nightly, releases, installer smoke, CUDA build) and fixtures.
  - Config: `.cargo/config.toml` (aliases `cargo xtask` and `cargo workshop`; Windows `rust-lld` and static CRT), `.config/` (nextest profiles, hakari), `.githooks/` (pre-commit fmt; pre-push headless check, clippy, deny; inactive locally because `core.hooksPath` is unset), `.cursor/rules/` (Workshop architecture and SPA rules), `deny.toml`, `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml`. `images/` holds README art and `target/` is build output.
- Component boundaries (from `cargo metadata` and AGENTS.md; `cargo test -p build-xtask` enforces them):
  - Engine: the `promptforge` facade depends on every `promptforge-*` crate; `engine` depends on `lua`, `parser`, `store`, `model-client`, `types`, `vfs`; `parser` on `lua` and `types`; `lua` on `model-client`, `store`, `types`; `store` on `vfs`; `model-client` on `types`; `types` and `vfs` are leaves. Internals may list `promptforge` only as a dev-dependency for doc examples. No edges to gateway, workshop, or harness crates; outside crates may name only `promptforge`.
  - Harness: the `harness` facade depends on `runner`, `sessions`, `log`; `sessions` on `models`, `runner`, `web`, `capabilities`, `log`; `web` on `webfetch`, `web-search`, `capabilities`; `models` on `runner`; `runner` on `capabilities` and `log`; `webfetch` and `web-search` on `capabilities`. Every `harness-*` crate's only outside dependency is `promptforge`; outside crates may name only `harness`.
  - Gateway: `gateway` (app) depends on `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `stt`, `web-search`, the public pair, and `shared-loopback`; `stt` on `local`, `config`, `progress`, `stt-engine`, `stt-backend-whisper`; `backend-whisper` on `whisper-ffi`, `stt-engine`, `progress`; `local` on `routing`, `protocol`, `progress`, `config`; `routing` and `web-search` on `protocol` and `config`; `protocol` on `config` and `gateway-api-types`. Never on promptforge, harness, or workshop crates; outside crates may name only the public pair.
  - Workshop tiers, each depending only downward: vocabulary (`workshop-protocol`, `workshop-registry`, `workshop-support`), services (`workshop-gateway`, `workshop-menu`, `workshop-status`), features (`workshop-user-state`, `workshop-workspace`), then `workshop-server`, which also uses `harness`, `promptforge`, `gateway-api-discovery`, and `shared-loopback`. `workshop-server-api` is the facade over the server, and the desktop app depends only on it plus `gateway-api-discovery`. Same-tier crates meet only through `workshop-protocol` types and `workshop-registry` slots; `workshop-gateway` uses only the gateway public pair.
  - Shared and `build-*` crates depend on no workspace crates; `build-*` is exempt from container privacy. Unsafe code is allowed only in `workshop` (desktop), `gateway` (app), `gateway-api-discovery`, and `gateway-whisper-ffi`.
  - At runtime the gateway is a separate process: the desktop app and server find it through the `gateway.json` discovery file and attach over HTTP and WebSocket, launching it as a sidecar when absent; vendor credentials never leave the gateway.
  - The archdoc lists a CLI component, but the workspace has no CLI crate or binary; the product binaries are `promptforge-gateway` and `promptforge-workshop`.
- Conventions summary:
  - Rust 2024 on stable with resolver 3; every dependency version lives in `[workspace.dependencies]` with a comment justifying each pin or feature choice, and every member depends on `workspace-hack`.
  - Workspace lints: `unsafe_code` forbidden outside the owned boundaries (a safety comment precedes every unsafe block); `missing_docs`, `unreachable_pub`, and `missing_debug_implementations` warn; clippy `all` and `pedantic` deny, plus `unwrap_used` and `expect_used`; broken or private intra-doc links deny.
  - Facade crates (`promptforge`, `harness`) hold only single-item `pub use` re-exports in role modules, with docs pulled from sibling `.md` files via `#![doc = include_str!(...)]`; Workshop `lib.rs` files are facade-only too.
  - Every `workshop-*` and `harness-*` `lib.rs` opens with a `//!` doc containing `## Invariants` that names allowed and forbidden dependencies; Rust files in those crates stay under 500 lines (split first, then edit).
  - Source directories are flat: one or two related files sit beside the parent as `foo-bar.rs` with `#[path = "foo-bar.rs"] mod bar;`, and three or more become a `foo/` subdirectory, converting in both directions.
  - Errors use `thiserror`; third-party causes are wrapped in `shared-error-source` newtypes (the harness owns its own); error and status messages are written for model readers: concise, factual, naming required versus actual.
  - Comments explain only non-obvious constraints, and every workaround cites its upstream issue URL.
  - Run-log and replay JSON round-trips exactly: serde_json `float_roundtrip`, sorted keys, finite numbers, never `preserve_order`.
  - Cargo features gate real constraints only; library and serve paths return errors instead of exiting or installing process-global state; harness crates spawn tasks only through `harness-runner`'s `spawn` helpers (raw `tokio::spawn` is banned per crate `clippy.toml`).
  - Reuse an existing facility before adding one; no new structural checks (parsers, allowlists, counts, import walkers) without explicit user approval; behavior changes ship with tests.
  - SPA: each feature directory keeps its `.ts` and `.css` together; component CSS uses `--ws-*` tokens from `@workshop/look` instead of raw values; no `localStorage` (state persists through the server's `ui-storage` adapter); lazy panels never import from the entry bundle; `*.contribution.ts` modules self-register at module scope.
  - Workshop server subsystems self-register into `workshop-registry` slots from the composition root `crates/workshop/server/src/app/compose.rs`.
  - Builds never write into the repository (CI fails on a dirty tree), and AGENTS.md fixes shared vocabulary (shell, desk, zone, chip, sidecar, publication, and others).

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Bound build-ui's upward searches at the install root [completed]

- Component: `none`

- Scope: W1, which fixes D1-1 and folds in D1-3 (todo `w1-install-root`). Land it as one commit that holds the code change and its tests and touches only `crates/build-ui/src/lib.rs` and `crates/build-ui/src/lib-tests.rs`.
- Failing-first tests: in `crates/build-ui/src/lib-tests.rs` (wired from `lib.rs` line 409 with `#[path = "lib-tests.rs"]`), add `an_install_above_the_install_root_is_not_used`, `a_workspace_root_above_the_install_root_is_not_adopted`, and `a_malformed_package_json_above_the_install_root_is_not_read` with the layouts and expectations the Testing Plan gives, using the existing `install_esbuild` helper (line 21) where a test needs an esbuild shim. Run `cargo nextest run --locked -p build-ui --lib above_the_install_root` and confirm all three fail before `lib.rs` changes.
- Code in `crates/build-ui/src/lib.rs`, as the Technical Design describes:
  - Add the private helper `install_root(ui_dir: &Path) -> PathBuf`, returning the directory that holds `nearest_lockfile(ui_dir)` (line 224), or `normalize(ui_dir)` (line 251) when there is no lockfile.
  - Bound `find_esbuild` (line 236) and `workspace_members` (line 184) so each walks `normalize(ui_dir)` and its ancestors up to and including `install_root(ui_dir)`, never above it.
  - Rewrite the `find_esbuild` doc comment (lines 231-234) to say where the search stops. Leave the module doc (lines 13-15) as written; its "there is no fallback" is true again.
  - Keep every signature, the missing-install error text, and the watch list that `watched_paths` (line 141) produces for both UIs unchanged. `esbuild_command` (line 321) may take its error directory from `install_root`.
- Existing tests in `lib-tests.rs`, as the Testing Plan lists:
  - `an_ancestor_install_is_found` (line 42) also writes `temp/package-lock.json`.
  - `no_install_in_the_tree_finds_nothing_in_it` (line 51) asserts `find_esbuild(&ui).is_none()` outright and drops the home-level comment.
  - `the_other_listed_members_are_returned`, `a_package_json_without_workspaces_is_not_the_root`, `a_malformed_package_json_is_an_error_naming_it`, `the_ui_dir_is_left_out_when_reached_through_a_parent_hop`, and `watch_covers_the_workspace_root_and_the_other_members` (lines 80-183) each write `package-lock.json` at the root they expect to find, and so does any other test that fails for the same reason.
- Out of scope: the stale comment at `lib.rs` line 157 and the Workshop's watch of `crates/shared-ui` (D1-6, deferred).
- Verify:
  - `cargo nextest run --locked -p build-ui`, `cargo test --doc -p build-ui`, `cargo clippy -p build-ui --all-targets -- -D warnings`, `cargo fmt --all --check`, and the structural checks `cargo test -p build-xtask` that every change runs.
  - `cargo nextest run --locked -p build-ui --test it --no-capture` prints no `skipping:` line, so the differential test `both_implementers_emit_the_same_layout` in `crates/build-ui/tests/it/main.rs` still runs.
  - With both installs present, `cargo build -p workshop-server` and `cargo build -p gateway-config-ui` succeed. Their install roots are `crates/workshop` and `crates/gateway/config-ui/ui`, where the two tracked `package-lock.json` files live.
  - Operator spot check: temporarily rename `crates/gateway/config-ui/ui/node_modules`, confirm `cargo build -p gateway-config-ui` fails with the setup message naming `crates/gateway/config-ui/ui`, then restore the folder.
  - Before committing, `git diff --stat HEAD` lists only the two `build-ui` files, so nothing under `crates/gateway/` or `crates/shared-ui/`. After building, testing, and committing, `git status` is clean.

</step-1>

<step-2>

### Step 2: Add the hover-color token and correct the theming sentences [completed]

- Component: `none`

- Scope: W2, which fixes D1-2 (todo `w2-look-theming`). Land it as one commit that touches only `crates/workshop/look/tokens.css`, `crates/workshop/look/controls.css`, `crates/workshop/look/AGENTS.md`, `AGENTS.md`, and `.cursor/rules/workshop-spa.mdc`. It is independent of Step 1: the two steps touch disjoint files, so either can land first.
- Palette token, as the Technical Design describes:
  - In `crates/workshop/look/tokens.css`, inside the "Accent and semantics" block (line 93), add `--accent-bright: color-mix(in srgb, var(--accent) 88%, #ffffff);` right after `--accent-dim` (line 95). Do not use `--accent-hover` or any other of the 13 deleted Gateway alias names.
  - In `crates/workshop/look/controls.css` line 48, `.button-primary:hover` uses `background: var(--accent-bright);`. The computed color does not change.
  - Leave the `tokens.css` header (lines 6-7) as written; with the token in place, its claim that no component stylesheet hardcodes a color holds for this color again.
- Theming sentences:
  - `crates/workshop/look/AGENTS.md` line 7: replace "A designer themes the family by editing `semantic.css`." with a sentence saying that colors, and every `look` component, are themed in `tokens.css`, and that `sizes.css` and `semantic.css` hold the size scale and the semantic aliases that Workshop UI component CSS reads as `--ws-*` tokens.
  - `AGENTS.md` line 89 and `.cursor/rules/workshop-spa.mdc` line 20, worded the same in both: scope the opening rule to Workshop UI component CSS, and replace "A designer themes the family by editing look/semantic.css." with a sentence saying that sizes and elevation are themed in `look/semantic.css`, and colors in `look/tokens.css`, which `look`'s own component sheets read directly.
- Out of scope: every Deferred item, including D1-12 and the pre-existing parts of D1-2. Add no phrase to `crates/workshop/ui/test/docs-claims.mjs` and no other new structural check.
- Verify:
  - `git grep -n "themes the family by editing" -- . ":(exclude)vibe/"` returns nothing. The exclusion skips planning records such as the tracked design record `vibe/2026-09-27-1-fork-shared-ui-look.md`, whose line 355 quotes the old sentence as history and stays as written.
  - `git grep -n "#ffffff" -- crates/workshop/look/controls.css` and `git grep -n -e "--accent-hover" -- crates/workshop` both return nothing.
  - In `crates/workshop`, run `npm run typecheck --workspaces --if-present`, `npm run build --workspace ui`, and `npm test --workspaces --if-present`, which cover `look/test/boundary.mjs` and `ui/test/docs-claims.mjs`. Then run the structural checks `cargo test -p build-xtask` that every change runs.
  - Visual check: the primary button's hover color in the Workshop is unchanged.
  - Before committing, `git diff --stat HEAD` lists only the five files above, so nothing under `crates/gateway/` or `crates/shared-ui/`. After building, testing, and committing, `git status` is clean.

</step-2>

</execution-plan>
