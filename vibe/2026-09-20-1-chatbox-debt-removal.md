---
name: Debt removal: chatbox extraction
overview: Remove the two debts the ChatBox extraction (37be5c6d..bbff0144 in promptforge2) introduced or worsened - an inert remove button and two overstated docstrings on the ChatBox contract (DEBT-CBX-001), and eleven dead CI cache globs plus one stale test header left by the package relocation (DEBT-CBX-002). Two small commits, focused tests only.
todos:
  - id: step-1
    content: "Step 1 (CBX-001): renderChip removable option, strip and static renderer pass false, reserved docstrings, restored-strip assertion"
    status: pending
  - id: step-2
    content: "Step 2 (CBX-002): delete 11 dead crates/workshop/*/ui/package-lock.json globs in 4 workflow files; fix test/mention-chip.mjs header"
    status: pending
isProject: false
---

# Debt Removal: ChatBox Extraction

<product-contract>

## Product Requirements

- Scope and target work: repository `c:/Users/Vinnie/cursor/promptforge2`, branch `vibe2`. Baseline `upstream/master` = `37be5c6d`; endpoint and disposition ref `bbff0144`; worktree clean. Target: the seven commits of plan `vibe/2026-09-19-1-chatbox-extraction.md` (`f72730f0`, `012b28a1`, `fcf543f5`, `03b60282`, `fa378289`, `f0c2b1bb`, `bbff0144`). Design records read: `vibe/archdoc.md` (no invariant touched), that plan.
- Goals: make the `types.ts` contract tell the truth about which seams are read; make every pill the live attachments strip paints button-free until removal ships; delete path globs that match no file; correct the one stale path literal the target left.
- Non-goals: implementing `onPasteFiles`, `commandSource`, or attachment removal; a relocation-wide path-literal checker (raised for the user, not approved); the `config-ui/ui/` relocation; any change to `ChatBoxHandle`, `SerializedDraft`, the wire, or component ownership.
- Success criteria: `test/chat-box.mjs` asserts a restored strip pill has no `.ws-mention-chip__remove`; `rg 'workshop/\*/ui' .github` returns nothing; `rg 'parts/agent/mention-chip' crates/workshop/ui/test` returns nothing; `types.ts` marks `onPasteFiles` and `commandSource` reserved in the wording `action` and `variant` already use; the focused tests and `npm run typecheck` pass.
- Constraints: focused tests only per the operator ("test the minimum, no full verify"); no interface or persisted-shape change; CSS class names unchanged.

## Functional Specification

- DEBT-CBX-001 (introduced, low): `chat-box.ts:576` paints the live strip with `renderChip(chip)` on `restore()`; `chip-view.ts:130-134` appends a `Remove` button to every pill that nothing wires there; `chip-view.ts:1-8` states callers either wire it (NodeView) or drop it (static renderer), and `restore()` does neither. `types.ts:108,110` document `commandSource` and `onPasteFiles` as behavior though neither is read; `types.ts:79,87` already use "reserved" for `action: "stop"` and `variant`. After the fix: a restored strip pill renders with no remove button; the NodeView pill still has one; the two docstrings say reserved and unread in this release.
- DEBT-CBX-002 (worsened, low): five relocations of the UI package in history; two corrective episodes (`4907c769` after the gateway reorg, target `f72730f0` repairing references left stale by `35bdbe62`). Residue at `bbff0144`: `crates/workshop/*/ui/package-lock.json` matches no file (lock is at `crates/workshop/ui/package-lock.json`) in 11 lines across `.github/workflows/ci.yml` (44, 93, 139, 171, 253, 306), `nightly.yml` (67, 101, 164), `release-workshop.yml` (118), `dist-ci/build-setup.yml` (15); `crates/workshop/ui/test/mention-chip.mjs:1` cites `src/parts/agent/mention-chip.ts`, which does not exist (regressed by `fcf543f5`). After the fix: no dead glob; header names `src/parts/chatbox/mention-chip.ts`. The surviving `crates/*/ui/package-lock.json` pattern continues to hash the workshop lock.
- Acceptance: the four grep and test checks under Success criteria; behavior of the NodeView pill and the static renderer unchanged.

</product-contract>
<implementation-contract>

## Technical Design

- `crates/workshop/ui/src/parts/chatbox/chip-view.ts`: `renderChip(chip: ChipRef, options?: { removable?: boolean }): HTMLElement`, default `removable: true` so the NodeView caller (`mention-chip.ts:184`) and the five test call sites are unchanged; when `false`, no `.ws-mention-chip__remove` button is created. Header comment names the third caller (the live strip) and its branch.
- `crates/workshop/ui/src/parts/chatbox/chat-box.ts:576`: `renderChip(chip, { removable: false })`.
- `crates/workshop/ui/src/parts/chatbox/chat-box-view.ts:16-20` `renderStaticChip`: pass `{ removable: false }` and delete the `querySelector(".ws-mention-chip__remove")?.remove()` line.
- `crates/workshop/ui/src/parts/chatbox/types.ts:107-110`: append to the `commandSource` and `onPasteFiles` docstrings a sentence in the file's idiom: "Reserved: declared but not read in this release; a typed `/` stays text." and "Reserved: declared but not read in this release; paste is ProseMirror's default." Type shape unchanged.
- `.github/workflows/ci.yml`, `nightly.yml`, `release-workshop.yml`, `dist-ci/build-setup.yml`: delete every `crates/workshop/*/ui/package-lock.json` line (11 total). Each `cache-dependency-path` block keeps `crates/*/ui/package-lock.json` and `crates/gateway/*/ui/package-lock.json`. `build-setup.yml` says "Keep in sync with the setup steps in ci.yml"; both change together.
- `crates/workshop/ui/test/mention-chip.mjs:1`: `src/parts/agent/mention-chip.ts` becomes `src/parts/chatbox/mention-chip.ts`.
- No interface, data, protocol, security, failure, or lifecycle changes. `renderChip` is not on `ChatBoxHandle`; the option is additive.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused (CBX-001): in `crates/workshop/ui/test/chat-box.mjs`, extend the existing `restore` assertions (around lines 1008-1015) to assert the restored strip pill has no `.ws-mention-chip__remove` descendant; `test/mention-chip.mjs` keeps its assertion that a NodeView pill has one (guards the default); `test/chat-box.mjs` line 1241's static-renderer assertion passes through the new option. Command: `node test/chat-box.mjs; node test/mention-chip.mjs` in `crates/workshop/ui/`, plus `npm run typecheck`.
- Focused (CBX-002): `rg -n 'workshop/\*/ui' .github` returns nothing; `rg -n 'parts/agent/mention-chip' crates/workshop/ui/test` returns nothing; `node test/mention-chip.mjs` still passes; `Test-Path crates/workshop/ui/package-lock.json` is true (the surviving `crates/*/ui/` pattern has a file to hash).
- Per the operator, no full-suite, build, formatter, linter, or docs run in this plan; FOCUSED scope on every step, including the last.
- Exit: `npm run typecheck`; the two grep checks; `git status --porcelain` empty after the commits.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions: `renderChip` gains an options object rather than exporting `renderStaticChip` (one rendering function, one flag; the static renderer loses its post-hoc DOM removal). Docstrings adopt the file's "reserved" wording rather than removing the props (the type surface the extraction plan promised stays stable). Dead globs are deleted rather than widened or replaced with a literal (a literal is one more path for the next relocation to miss).
- User-resolved architecture choices: none required.
- Raised, not proposed: a path-literal resolver test over `.github/`, `.cursor/rules/`, READMEs, and test headers. It is a source-text ratchet protecting a maintenance habit rather than a product contract; it needs explicit approval.
- Rejected candidates (12): residual-but-acceptable 7 (`build_sibling("../ui")`, `build()` shim, oversized `setupStt` and `ChatBox.constructor`, `parsePayload` null on bad clipboard JSON, structural mirrors `ChatBoxTextControl`/`ChatBoxHandle`, reserved seams, registry lookup moved to the view); weak/speculative 3 (duplicated test helpers and 160 ms settle, `busy` only during `recording`, raw ProseMirror JSON on a persisted boundary with nothing persisting yet); false 2 (interim `this.mic` listener already removed, CI cache path still matched by `crates/*/ui/`).
- Risks: none of the changes is observable by an operator today; no production caller of `restore()` exists at `bbff0144`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build -p workshop-server` (runs `build.rs` -> `build-ui::build`, which resolves the UI package at `<manifest_dir>/ui` and bundles into `$OUT_DIR/ui-dist`; needs Node 22+ and one `npm ci` in the UI package). Standalone UI bundle: `npm run build` in `crates/workshop/server/ui` (`node build.mjs`, esbuild, ESM, splitting, minified, outputs `dist/`). Toolchain present locally: Node v24.19.0, npm 11.17.0, cargo 1.98.0 (stable channel per `rust-toolchain.toml`), cargo-nextest 0.9.128. Windows builds use `rust-lld` and static CRT via `.cargo/config.toml`.
- Focused test command pattern: UI (jsdom, node:test): `node test/<name>.mjs` from `crates/workshop/server/ui` (each test file bundles its subjects through an esbuild `stdin` block listing `./src/...` paths, so moved source files require editing those stdin import strings). Rust: `cargo nextest run --locked -p workshop-server --features test-fixtures --test it <filter>` (e.g. `chat_gate`); `cargo test -p build-xtask` for the structural harness; `cargo test -p build-ui` for the Node-vs-Rust bundle drift test (`both_implementers_emit_the_same_layout`, hardcodes `../workshop/server/ui` from `crates/build-ui`; skips when node is absent or `node_modules` is missing).
- Component test command pattern: UI package: `npm run typecheck` (`tsc --noEmit`, strict, `noUncheckedIndexedAccess`, `verbatimModuleSyntax`, `moduleResolution: bundler`) then `npm test` (`node --test "test/**/*.mjs" "src/**/*.test.mjs"`) in `crates/workshop/server/ui`. Workshop Rust partition: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; headless variant `cargo nextest run --locked -p workshop-server --features headless`; doctests `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the workshop partition command above, then `npm test` in the UI package. CI (`.github/workflows/ci.yml`) also runs `cargo check -p gateway --no-default-features`, `cargo test -p gateway-stt --test it architecture`, and a clean-tree check (`git status --porcelain` must be empty after builds).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop partition: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Workspace lints deny `clippy::all`, `clippy::pedantic`, `unwrap_used`, `expect_used`; `unsafe_code` is forbidden; `missing_docs` warns. No JS/TS linter (no eslint/biome config found); TypeScript strictness is the only TS gate. Pre-push hook runs headless check, clippy, and `cargo deny check` when available.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook; `rustfmt.toml` present). No JS/TS formatter configured (no prettier/biome config found).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; user guide: `mdbook build guide`.
- Test placement and naming conventions: UI tests are flat `.mjs` files in `crates/workshop/server/ui/test/` (kebab-case, named after the subject: `prompt-input.mjs`, `agent-session-view.mjs`, `agent-stt.mjs`, `typeahead-popup.mjs`, `mention-chip.mjs`, `speech-capture.mjs`, `stt-stream.mjs`), each opening with a comment block describing coverage and a `// Run: node test/<name>.mjs` line; shared fixtures in `test/helpers/` (`boot.mjs`, `leak-check.mjs`, `bundle-seams.mjs`, `lazy-feature.mjs`, tauri stubs, `ui-storage.mjs`). Tests import subjects via an esbuild stdin bundle rooted at the package (`resolveDir: test/..`), run in jsdom, and many use `assertNoLeaks` (undisposed disposables fail). `test/no-local-storage.mjs` forbids `localStorage`; bundle guard tests enforce lazy-chunk boundaries. The glob also accepts `src/**/*.test.mjs` colocated tests but none exist today. Rust integration tests live in `crates/workshop/server/tests/it/` (one `main.rs` target, one module per subsystem, subdirectories `chat_gate/`, `agents/`, `realtime_relay/`, etc. with three-plus files each; `tests/common/mod.rs` shared); unit tests inline in `src`. Repo rule: a source subdirectory needs three or more files, else use `foo-bar.rs` kebab siblings with `#[path]`.
- Directory map: `Cargo.toml` (workspace, resolver 3, edition 2024, `default-members = crates/gateway/app`; excludes `crates/shared-ui` and the manifestless containers) / `crates/` - root public layer: `build-ui` (esbuild-driving build helper plus drift test), `build-xtask` (structural harness), `build-workshop`, `build-llama-cuda`, `build-user-guide`, `gateway-api`, `gateway-api-discovery`, `promptforge-api-runtime`, `promptforge-api-types`, `shared-loopback`, `shared-progress`, `shared-vfs`, `shared-ui` (TypeScript+CSS package, not a crate; `file:` dependency of both UIs, exports `tokens.css`, `controls.css`, `modal`, `dropdown`, `toast`, `status-bar`, `progress`), `workspace-hack` (hakari); `crates/promptforge/` (private family: lua, parser, store, vfs, model-client, web, webfetch, web-search); `crates/gateway/` (private family: app, cloud-providers, config, config-ui with its own `ui/` npm package, local, logging, protocol, routing, web-search, `stt/` subsystem); `crates/workshop/` (private family: `shell` = package `workshop` (Tauri), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `sessions`, `status`, `support`, `user-state`, `workspace`). `crates/workshop/server/` holds `build.rs`, `src/`, `tests/`, and the `ui/` npm package (`workshop-ui`: `package.json`, `package-lock.json`, `build.mjs`, `index.html`, `style.css`, `pcm-worklet.js`, `icons/`, `tsconfig.json`, `AGENTS.md`, `THIRD_PARTY_NOTICES.md`, `src/`, `test/`, `node_modules/`). `ui/src/`: `main.ts` (composition root), `base/` (`event.ts`, `lifecycle.ts`, `paths.ts`, `workshop-part.ts`), `services/` (DOM-free registries and services incl. `panel-registry.ts`, `service-registry.ts`, `speech-capture.ts`, `text-control-service.ts`, `model-service.ts`, `agent-session.ts`), `tokens/` (`base.css`, `semantic.css`, `component.css`), `ui/` (feature dirs: `agent/`, `chrome/`, `editor/`, `gateway/`, `layout/`, `menu/`, `quickinput/`, `run/`, `shared/`, `status/`, `stt/`, `take/`, `workspace/`, `workspace-files/`, plus `workbench.contributions.ts`). `ui/src/ui/agent/` today: `agent-panel.ts`, `agent-session-view.ts` + `.css`, `agent-toolbar.ts` + `.css`, `agent-menu.ts`, `agent.contribution.ts`, `index.ts`, `markdown-render.ts` + `.css`, `mention-chip.ts`, `mode-chip.ts` + `.css`, `prompt-input.ts` + `.css`, `tool-call-card.ts` + `.css`, `typeahead-popup.ts` + `.css`. `ui/src/ui/stt/`: `index.ts`, `realtime-stt.ts`, `stt.ts`, `stt.css`. Other root items: `.github/workflows/` (`ci.yml` and release/nightly workflows; `ci.yml` names `crates/workshop/server/ui` in `npm ci --prefix` lines and `working-directory` of the `ui` job, plus `crates/workshop/*/ui/package-lock.json` cache paths), `.githooks/` (`pre-commit`, `pre-push`), `.cargo/config.toml` (aliases `workshop`, `xtask`), `.config/` (`nextest.toml`, `hakari.toml`), `.cursor/rules/` (`workshop-spa.mdc` with glob `crates/workshop-server/ui/**`, `workshop-architecture.mdc`), `guide/` (mdbook), `tools/` (`stage-gateway-sidecar.mjs`, tts scripts), `vibe/archdoc.md`, `AGENTS.md`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `.gitattributes`, `.gitignore`.
- Component boundaries: dependency direction is shell -> features -> services -> vocabulary; in the SPA, `ui/` -> `services/` -> `base/`, never reversed; `main.ts` is the composition root and nothing imports it; lazy feature directories (loaded via dynamic `import()` from `services/panel-registry.ts`) never import the boot shell; each feature `index.ts` exports only `register()` and never `export *`; contribution files (`ui/<feature>/<feature>.contribution.ts`) register at module scope and lazy-import heavy deps (tiptap, CodeMirror, dockview, Shiki) so the entry bundle stays lean (enforced by bundle guard tests). Product rule: workshop crates never depend on gateway crates except the public `gateway-api`/`gateway-api-discovery`; family containers are private; `workshop` shell depends on `workshop-server-api`, never `workshop-server`; `build-*` crates are exempt meta tooling. `workshop-server` depends on `build-ui` as a build-dependency and reaches the UI package by `manifest_dir.join("ui")` (moving the package requires changing that resolution, the `build-ui` drift test path, `build.mjs`'s four-level parent walk to `Cargo.toml`, and the `shared-ui` `file:../../../shared-ui` link). Chat composer today: `agent-session-view.ts` owns `PromptInput` (tiptap in `prompt-input.ts`), `mention-chip.ts`, `typeahead-popup.ts`, and `agent-toolbar.ts`; `stt/stt.ts` (`setupStt`, `SttInputTarget`) and `stt/realtime-stt.ts` drive dictation against a shared `services/speech-capture.ts` `SpeechCaptureService` registered under `SPEECH_CAPTURE`; status messages go through `ui/status/status-bar.ts` (`STATUS_BAR`). Rust: `chat_gate` integration tests in `crates/workshop/server/tests/it/chat_gate/` exercise the server side of the pending-input gate.
- Conventions summary: TypeScript ES2022 ESM, strict tsconfig, kebab-case files and directories, CSS colocated beside its `.ts` and imported as a side effect, `.ws-` class prefix and `--ws-*` token-only values (no raw colors/sizes in component CSS), no `localStorage` (persist through `ui-storage` to server-side allow-listed keys), state in services with change emitters passed through constructors (no mutable module globals), disposables via `base/lifecycle.ts` with leak checks in tests, VS Code command ids and context keys reused verbatim, `registerAction` for commands/menus/keybindings, stub menu rows in `ui/menu/stubs.contribution.ts`. Rust: edition 2024, no `unsafe`, no `unwrap`/`expect`, pedantic clippy, `missing_docs` warned, every workshop-* `lib.rs` opens with a `//!` doc carrying `## Invariants`, no file over 500 lines (enforced by `build-xtask` for marker-bearing files), comments only for non-obvious constraints with upstream issue URLs for workarounds, behavior changes ship with tests in the same change, structural checks need explicit approval, error messages written for model consumption. Git: history-preserving moves via `git mv`; no build step may write into the repository (UI bundles go to `OUT_DIR`; `dist/` under the UI package is a local artifact). Current branch is `vibe2` with a clean tree.

</project-survey>
<execution-plan>

## Execution Instructions

Bounded path: two steps, Component `none`. Focused verification only, per the operator.

<step-1>

### Step 1: CBX-001 - removable option on renderChip and reserved docstrings [completed]

- Component: none
- Depends on: none
- Work: `chip-view.ts` `renderChip(chip, options?: { removable?: boolean })`, default true, no remove button when false, header comment names the live strip as the third caller; `chat-box.ts:576` passes `{ removable: false }`; `chat-box-view.ts` `renderStaticChip` passes `{ removable: false }` and drops the `querySelector(...).remove()` line; `types.ts` `commandSource` and `onPasteFiles` docstrings gain the reserved sentence.
- Tests: `test/chat-box.mjs` restore block asserts the restored strip pill has no `.ws-mention-chip__remove`. Gate: `npm run typecheck`; `node test/chat-box.mjs`; `node test/mention-chip.mjs` (in `crates/workshop/ui/`).
- Commit: `chip-view.ts`, `chat-box.ts`, `chat-box-view.ts`, `types.ts`, `test/chat-box.mjs`.

</step-1>

<step-2>

### Step 2: CBX-002 - delete dead lockfile globs and fix the test header [completed]

- Component: none
- Depends on: none
- Work: delete the 11 `crates/workshop/*/ui/package-lock.json` lines from `.github/workflows/ci.yml`, `nightly.yml`, `release-workshop.yml`, `dist-ci/build-setup.yml`; change `crates/workshop/ui/test/mention-chip.mjs` line 1 to `src/parts/chatbox/mention-chip.ts`.
- Tests: `rg -n 'workshop/\*/ui' .github` empty; `rg -n 'parts/agent/mention-chip' crates/workshop/ui/test` empty; `node test/mention-chip.mjs` passes. No failing-test-first shape exists (comment and YAML edits).
- Commit: the four workflow files and the test header.

</step-2>

</execution-plan>
