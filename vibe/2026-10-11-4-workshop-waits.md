---
name: Workshop waits under parallel calls
overview: "Remove the debt that parallel tool calls exposed in the Workshop agent session service: an open operator question must keep the composer answerable when a sibling call lands, and every open question must stay reachable, oldest first. The guide's stale fanout examples are recorded but left out at the user's direction."
todos:
  - id: ui-waits
    content: "D1-1 and C1-1: track open waits oldest first in agent-session.ts, gate the tool_call_update generating flip on no open wait, and add the service and view regression tests"
    status: completed
  - id: verify
    content: Run npm test and typecheck in crates/workshop and the docs-claims scan; confirm each new test fails at 83d40b1b9
    status: completed
isProject: false
---

# Keep Workshop waits correct when a round's calls run in parallel

<product-contract>

## Product Requirements

This plan fixes two Workshop UI defects that parallel tool calls made reachable in the ordinary one-loop agent shape. Both live in one service file and are fixed together with regression tests. The debt the target itself introduced, a published guide that still teaches the removed `fanout` global, is recorded but left out of this plan at the user's direction.

- Scope and target work:
  - Target: commits `1ce5831d0` (Run one round's catalog tool calls in parallel), `a490542e9` (Move fanout into the tasks table as tasks.fanout), and `83d40b1b9` (Close plan: parallel-tool-calls), all with `Plan: vibe/2026-10-11-3-parallel-tool-calls.md`.
  - Baseline `e62bd6d77`, the merge base of `master` with `origin/master`. Endpoint and disposition `83d40b1b9`, with a clean worktree.
  - Removal scope: D1-1 and C1-1, which the target exposed in Workshop UI code it did not touch, brought into scope by the user.
- Cleanup goals:
  - While an operator question from the model is open, the composer stays answerable, whatever the round's other calls do.
  - When the model asks two or more questions in one round, every one stays reachable, and the composer answers them oldest first.
- Non-goals:
  - No Engine, Harness, Workshop server, or wire-format change.
  - No edit to `../runvil-docs`.
  - No change to what Stop means while a question is open (D1-12).
- Success criteria:
  - Each new regression test fails at `83d40b1b9` and passes after the change.
  - Every existing Workshop UI test, the UI type checks, and the wording scan pass. Four existing checks in the model-ask block keep their assertions, but their setup clears a wait the shipped UI could never leave open (Testing Plan).

## Functional Specification

The inventory separates the debt the target added, which is excluded by user decision, from the two exposed defects this plan fixes.

### Debt Inventory

- Debt added:
  - D1-2, introduced by `a490542e9`: the published language guide still teaches the removed `fanout` global.
    - Evidence: at the endpoint `fanout` lives only on the `tasks` table (`crates/runvil-internal/lua/src/coro.rs` line 264) and `RESERVED_NAMES` has 60 entries (`crates/runvil-internal/lua/src/globals.rs`). `../runvil-docs` at `44ff253` has 32 bare `fanout(` calls across chapters 01, 02, 07, 08, 09, 14, and 17, and no `tasks.fanout`. `.github/workflows/site.yml` publishes that checkout beside this repository.
    - Relationship: the plan's claim that "No other crate, `prompts/` file, or guide calls `fanout`" covered only the in-repo `guide/`, which holds no chapters.
    - Impact: once the target is pushed, every guide example of `fanout` fails at run time with a call to a nil global.
    - Reversal cost: low, documentation only. Target state: the guide calls `tasks.fanout(worker, collection)`.
    - Disposition: excluded by user decision. See Deferred and Out of Scope.
- Exposed debt, brought into scope by the user:
  - D1-1: a sibling call's result turns "generating" back on while the model's own question is open.
    - Evidence: `1ce5831d0` issues every leading catalog call in one step (`crates/runvil-internal/engine/src/execute/scheduler/tool_call.rs`, `dispatch_tool_calls`), so `user-input/ask` and a sibling such as `web/fetch` are in flight together, and each sibling's `ToolResult` fires when it lands. In `crates/workshop/ui/src/services/agent-session.ts`, the `tool_call_update` case (lines 456-464) turns generating on for any result that `answersModelCall` (lines 601-615) matches, without looking at the pinned wait. The `generating` getter's doc (lines 299-311) says it is off while a wait is open.
    - Impact: the composer action becomes Stop (`crates/workshop/ui/src/parts/agent/agent-session-view.ts` lines 279-291), Enter does nothing (`crates/workshop/ui/src/parts/chatbox/chat-box.ts` lines 522-526), and the operator must press Stop to reach Send. If another sibling is still running, that Stop drops it, the batch answers with the cancelled error, and the round's records, including the operator's answer, are discarded.
    - Reachability: an operator-authored agent that offers `user-input/ask` to the model, a supported shape. The built-in chat agent does not.
    - Reversal cost: low, local to the UI service.
  - C1-1: two model questions in one round strand the first.
    - Evidence: two `user-input/ask` calls in one round both issue at once. Each opens its own wait and sends `WaitFrame::Required` (`crates/workshop/agents/src/input-tool.rs` lines 89-103), and the server registry holds many waits in creation order (`crates/workshop/agents/src/input.rs` lines 81-87 and 120-131). The service keeps one pinned token, and `onInputRequired` overwrites it (`agent-session.ts` lines 259-279 and 632-648). After the operator answers, `respond` clears the pin (lines 340-375), and no frame re-announces the first wait until a reattach (`crates/workshop/server/src/agents/socket.rs` `resend_unresolved`).
    - Impact: the run stays parked on the unanswered first question, and the composer has no token to answer it. `pendingInputToken` is documented as "The pending wait's token, or null while the agent is working", yet it returns null while a wait is open.
    - Reversal cost: low, local to the UI service.
- Cheap fixes: none.
- Unrelated pre-existing debt: D1-17, the guide still calls `list_from_section`, which `d24da1d75` removed before the baseline. Excluded with D1-2.
- Rejected candidates: 15.
  - Residual but acceptable, 9 (D1-3 to D1-8, D1-11, D1-12, D1-14): exhaustive protocol and dispatch arms, a private correlated parameter cluster, batch slots whose lifecycle every path enforces, reset lists updated in both places, a rename complete inside the repository, sibling store identities recorded as a known trade-off, Stop beside a call that survives stops as a semantic choice, and one stale doc phrase.
  - Weak or speculative, 4 (D1-9, D1-10, D1-13, D1-15): duplicated routing checks with no reaching path, no contract that bounds calls per round, effect ids that shift only in multi-chain runs with no replayer, and a test-only single-call route.
  - False, 1 (D1-16): teardown's orphans keep the run's scope open on purpose.

</product-contract>
<implementation-contract>

## Technical Design

The agent session service tracks every open operator wait in the order the server announced it, instead of one pinned token, and stops treating a sibling call's result as proof the agent is working while a wait is open. The view, server, and wire format stay as they are.

- In `crates/workshop/ui/src/services/agent-session.ts`:
  - Open waits: keep the open wait tokens in announcement order. `pendingInputToken` returns the oldest open token, or null when none is open, which matches its doc.
  - `onInputRequired(token)`: add the token unless it is already open, since a reattach resend repeats tokens. Keep today's closing of thinking, clearing of `stopped`, and turning generating off, then pin the oldest open token.
  - `respond`, on success: remove the spent token, pin the next oldest, and turn generating on only when no wait remains open. A failed respond keeps every token, as today.
  - `onInputCancelled(token)`: remove that token wherever it sits, and pin the next oldest.
  - The attach acknowledgement: clear every open token before the server's `resend_unresolved` re-announces the unresolved waits in creation order, so the oldest is pinned again.
  - `tool_call_update` (lines 456-464): turn generating on only when `answersModelCall(id)` holds and no wait is open.
  - `setPinnedToken` (lines 632-648) stays the one place that publishes the pin, fed from the oldest open token, so the view's subscription is unchanged.
- Unchanged: `agent-session-view.ts` and `chat-box.ts`, which already read `generating` and `pendingInputToken`, the Workshop server and its frames, and the Engine and Harness.

</implementation-contract>
<verification-contract>

## Testing Plan

Each defect gets a regression test that fails at `83d40b1b9`. The existing pin, model-ask, and Stop tests are the regression suite for the paths this plan keeps.

- D1-1, in `crates/workshop/ui/test/agent-session-service.mjs`, beside the model-ask block (lines 490-506), using `makeWire` and `AgentSessionService` as that block does:
  - New test: fire `tool_call` for `[c1 user-input/ask, c2 web/fetch]`, then `inputRequired("tok1")`, then `tool_call_update` for `c2`. Expect `generating === false` and `pendingInputToken === "tok1"`. Then `respond` turns generating on.
  - Fails before the change, because the update turns generating on.
- D1-1, in `crates/workshop/ui/test/agent-session-view.mjs`, beside the Stop tests (lines 1060-1085): under the same sequence, the composer's `data-action` stays `send`.
- C1-1, in `agent-session-service.mjs`, beside the pin tests (lines 266-294):
  - Two open waits pin the oldest and advance: `inputRequired("tok1")` and `inputRequired("tok2")` leave `pendingInputToken === "tok1"`. Responding to `tok1` pins `tok2` with generating still off, and responding to `tok2` leaves null with generating on. Fails before the change, which pins `tok2` first and null after one respond.
  - Cancelling a wait removes it wherever it sits: `tok1`, `tok2`, then `inputCancelled("tok1")` leaves `tok2`, and in a second fixture `inputCancelled("tok2")` leaves `tok1`. Only the second case fails before the change, which already pins `tok2` by overwrite.
  - A reattach pins the oldest unresolved wait: `tok1`, `tok2`, the attach acknowledgement, then both `inputRequired` frames again leave `tok1`, with no duplicate token.
- Existing tests that must pass unchanged: "input_required pins", "a spent token unpins", "input_cancelled unpins", "an open wait keeps generating off", and the view's Stop tests (lines 1303-1342).
- Existing checks whose setup changes, in the model-ask block of `agent-session-service.mjs` (lines 490-525): "a tool_call_update answering a model batch call turns generating on", "a result for the current turn's call turns generating on", "an answered call turns it back on", and "a same-session acknowledgment leaves generating alone". Their setup leaves `tok1` and then `tok2` open when the tool results land, a state the shipped UI cannot reach. Each wait is cleared before its result lands, without turning `generating` on (for example with `inputCancelled`, not `respond`), so every check still sees a tool result turn `generating` from off to on. Their assertions stay as they are.
- Exit checks, in `crates/workshop`: `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present`. From the repository root: `node --test crates/workshop/ui/test/docs-claims.mjs`.

</verification-contract>
<decision-record>

## Decision Record

The fix stays inside the UI service, mirroring the server's existing wait registry. The user brought the two exposed defects into scope and kept the separate docs repository out of it.

- Reversible decisions and consequences:
  - Track open waits in the service in announcement order and pin the oldest. Consequence: the composer answers questions in the order the model asked them, which matches the server registry and its resend order.
  - Gate the `tool_call_update` generating flip on no open wait. Consequence: with no wait open, a landed sibling still turns generating on, as today.
  - Edit the setup of four existing model-ask checks, not the gate. Raised when the coding sub-agent found that those checks leave a wait open when a tool result lands. Evidence: wait frames carry only a token, with no call id (`crates/workshop/agents/src/input.rs`, `crates/workshop/ui/src/services/agent-socket.ts` lines 249-253). In a single client, `respond` clears the pin before the ask's result arrives. The shipped desktop UI gives each agent panel its own socket and session, and uses attach only to reconnect (`crates/workshop/ui/src/parts/agent/agent-panel.ts`, `crates/workshop/ui/src/services/agent-socket.ts`). So no shipped flow lands a result under a wait this client still holds open. Falsifier: a shipped flow that attaches two sockets to one conversation at once. There, a peer whose question was answered elsewhere would keep `generating` off until the next reply, and the precise fix would carry the ask's call id on the wait frame, a wire change.
  - Rejected: exempt an ask's own result from the gate. Two asks in one round would then turn `generating` on while the second question is still open.
- User-resolved choices:
  - Scope expanded to D1-1 and C1-1. User selected "D1-1 and C1-1: fix the two Workshop UI bugs in agent-session.ts".
  - `../runvil-docs` is not edited. User: "leave runvil-docs ALONE". This governs over the user's selection of the D1-17 guide pass, which edits the same repository, so D1-2 and D1-17 are out of scope.
- Rejected alternatives, assumptions, and risks:
  - The server resends the remaining waits after each accepted response. Rejected: it adds a second resend trigger and server traffic, and the UI must still handle two open waits on attach.
  - Turn generating on only when every call of the batch has a result. Rejected: it needs per-batch bookkeeping, and with no wait open, a landed sibling does mean the agent is working.
  - Run calls that survive stops alone in the Engine. Rejected: the Engine would need a Host concept, and it would undo the rule that a catalog call's route never depends on its round.
  - Assumption, inferred from code and not observed: the ask's wait usually opens before a network sibling lands. The fix holds in either order.
  - Risk: D1-12 remains. A deliberate Stop while a question is open still drops running siblings and discards the round. Fixing D1-1 removes the forced Stop that made this routine.

### Deferred and Out of Scope

- D1-2, the guide's 32 bare `fanout(` calls in `../runvil-docs`. Excluded by the user. Revisit before pushing `a490542e9`, because `.github/workflows/site.yml` publishes the guide beside the Engine, or whenever that repository is next updated.
- D1-17, the guide's `list_from_section` calls. Excluded with D1-2, with the same revisit condition.
- A durable guard against guide drift after author-facing renames. Declined by the user. Revisit at the next author-facing rename.
- D1-9, one shared route classifier for `runs_alone` and `prepare_tool_call`. Revisit when the subagents plan edits tool-call routing.
- D1-13, effect ids that shift in multi-chain runs. Revisit when a replayer is built.
- D1-10, a cap on calls in flight per round. Revisit if provider rate limits appear.
- D1-11's `ToolContext` doc sentence on sibling concurrency, and D1-14's stale `tool_call` phrase in `crates/runvil-internal/lua/src/protocol.rs`. Revisit at the next edit of those files.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway`.
  - `crates/gateway/app` is the workspace's only default member, so plain `cargo build` builds the same thing. The desktop app builds with `cargo workshop`, an alias for `run -p build-workshop --` in `.cargo/config.toml`.
  - The Workshop UI bundle builds with `npm run build --workspace ui` in `crates/workshop`, which runs esbuild through `ui/build.mjs`. The `workshop-server` and `gateway-config-ui` build scripts run the same bundling, so workspace-wide cargo commands need Node.js plus the npm installs in `crates/workshop` and `crates/gateway/config-ui/ui`. Both `node_modules` trees are present on this machine, along with Node.js 24.19 (CI uses 22), cargo-nextest 0.9.128, stable Rust, and `nightly-2026-09-05`.
  - Workshop clippy and tests need the gateway sidecar staged for Tauri's `externalBin`: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`, and `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc` afterward. `crates/workshop/desktop/binaries/` is empty now. The UI test suites do not need the sidecar.
- Focused test command pattern:
  - Workshop UI: `node test/<topic>.mjs` from `crates/workshop/ui`, such as `node test/agent-session-service.mjs` or `node test/agent-session-view.mjs`. Each file is a standalone script and the smallest runnable unit, with no per-assertion filter. Both of those pass at `83d40b1b9`. The `look` and `platform` packages run their files the same way from their own directories.
  - Rust: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`, which CI never builds with all features.
- Component test command pattern:
  - Workshop UI: `npm test --workspace ui` in `crates/workshop`, which runs `node --test "test/**/*.mjs" "src/**/*.test.mjs"`. The second glob matches nothing today, and the first also picks up `test/helpers/`. `--workspace @workshop/look` and `--workspace @workshop/platform` run the other two packages.
  - Rust: `cargo nextest run --locked -p <crate> --all-features`, with the same three exceptions. `cargo test -p build-xtask` runs the boundary and structural checks.
- Full-suite test command:
  - Workshop UI: `npm test --workspaces --if-present` in `crates/workshop`, plus `npm test` in `crates/gateway/config-ui/ui`.
  - The wording and rulebook scan: `node --test crates/workshop/ui/test/docs-claims.mjs` from the repository root. It needs no `npm ci` and runs from any directory.
  - Rust: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` with the sidecar staged. CI also runs `-p workshop-workspace --all-features`, `-p workshop-server --features headless`, and five launch-lease tests by name.
- Linter command:
  - Workshop UI: no ESLint or other JavaScript linter is configured, so the type check is the gate: `npm run typecheck --workspaces --if-present` in `crates/workshop`, which runs `tsc --noEmit` in `ui`, `look`, and `platform` (TypeScript 7, `strict`, `noUncheckedIndexedAccess`, `verbatimModuleSyntax`). All three pass at `83d40b1b9`. The `ui` tsconfig covers only `src`, so the `.mjs` tests are not type-checked. The gateway config UI runs `npm run typecheck` in `crates/gateway/config-ui/ui`.
  - Rust: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny` (in PowerShell, `$env:CARGO_BUILD_WARNINGS='deny'`). Never run a standalone `cargo check --workspace` beside them; the one exception is `cargo check -p gateway --no-default-features`.
  - Supply chain, run in CI: `cargo deny check`, `cargo audit`, `cargo hakari verify`, and a `cargo tree` check that `ring` stays out of the gateway's normal closure.
  - The pre-push hook in `.githooks/` (set as `core.hooksPath`) runs the wording scan, the headless gateway check, the non-Workshop clippy run, and `cargo deny` when installed.
- Formatter check command: `cargo fmt --all --check`, with `style_edition = "2024"` in `rustfmt.toml`, which the pre-commit hook runs. No formatter is configured for the TypeScript sources or `.mjs` tests, so new code matches the surrounding style by hand.
- Docs command:
  - Each with `RUSTDOCFLAGS="-D warnings"` (in PowerShell, `$env:RUSTDOCFLAGS='-D warnings'`): `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p runvil --no-deps`, `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
  - The facade surface check: `cargo +nightly-2026-09-05 xtask api --check` against `crates/runvil/public-api.txt`, with the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`.
  - The Workshop UI has no generated docs. Its module and test-file header comments are the documentation.
- Test placement and naming conventions:
  - Workshop UI tests live flat in `crates/workshop/ui/test/` as one `<topic>.mjs` per subject (113 files), with shared modules such as `leak-check.mjs` and `boot.mjs` in `test/helpers/`. `look/test/` (9 files) and `platform/test/` (10 files) use the same flat placement, and some of their files assert with `node:assert/strict` instead.
  - A UI test file opens with a comment stating what it covers, usually ending `Run: node test/<file>.mjs` (65 files). It bundles the TypeScript under test with esbuild from an inline `stdin` entry resolved from the `ui` directory, writes the bundle to the OS temp directory, and imports it. DOM tests run under jsdom (73 files) with `.css` loaded as empty.
  - Assertions go through a local `check(name, condition)` into a `failures` list (85 files), not `node:test` cases. The file prints `<file>: all assertions passed`, or lists every failing name and exits 1. Check names are lowercase sentences stating the behavior, such as "a spent token unpins" and "input_required pins its token".
  - Cases are scoped `{ }` blocks under `// --- Title ---` banners. Each builds its own fixtures, such as `makeWire()` and a fresh `AgentSessionService` in the agent-session tests, and disposes what it built. `assertNoLeaks` from `test/helpers/leak-check.mjs` checks disposal.
  - Rust unit tests mostly sit in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (about 176 files). A module whose tests reach three files keeps them in a `tests/` subdirectory beside it. Inline `mod tests { }` blocks remain in 141 files.
  - Rust integration tests compile as one binary per crate, from `tests/it/main.rs` in 18 crates and `tests/suite/main.rs` in `harness` and `runvil`, one module per topic with helpers in `support.rs`. Test names are snake_case sentences stating the behavior. Doctests are banned by `build-xtask`'s `no_doctests` check. `.config/nextest.toml` puts `gateway-stt` and `gateway-stt-backend-whisper` in a `heavy` test group.
- Directory map:
  - `crates/`: all 52 Cargo packages. The top level holds the public and shared layer: the `runvil` facade, `runvil-plugin`, `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`, the `plugin-*` crates, the `shared-*` crates, the `build-*` tooling, and `workspace-hack`.
  - `crates/runvil-internal/`: the Engine's private crates `types`, `engine`, `lua`, `parser`, `vfs`, and `model-client`. `crates/harness-internal/runner`: the Harness's private per-run runner.
  - `crates/gateway/`: the gateway's private crates, plus the `stt/` subsystem.
  - `crates/workshop/`: Workshop's Rust crates `desktop` (package `workshop`), `server`, `server-api`, `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, and `workspace`, plus the npm workspace root `package.json` holding the packages `ui`, `look`, and `platform`.
  - `crates/workshop/ui/src/`: `base/` (paths), `services/` (30 service modules, including `agent-session.ts`, `agent-socket.ts`, and `protocol.ts`), `parts/` (14 feature directories, including `agent/` and `chatbox/`, plus `workbench.contributions.ts`), `tokens/` (component CSS tokens), and `main.ts`, the entry. `ui/test/` holds the UI tests.
  - `crates/workshop/look/`: the shared look (palette, size and semantic tokens, icons, and behavioral components). `crates/workshop/platform/`: the shared UI mechanics (lifecycle, events, registries, `WorkshopPart`).
  - `crates/shared-ui`: a TypeScript and CSS package the gateway config UI uses, excluded from the Cargo workspace.
  - `guide/`: the `books`, `chrome`, and `landing` pieces of the mdBook site; the chapters live in the separate `runvil-docs` checkout. `prompts/`: five example prompts. `tools/`: Node scripts for staging the gateway sidecar and a live TTS check, each with a `.test.mjs`. `images/`: README art.
  - `vibe/`: plan records, recent ones at the top level and older ones filed by month (`2026-07` through `2026-10`), plus `scratch/`.
  - `.github/workflows/`: `ci.yml`, whose `ci-green` job is the single gate (its `ui` job runs the UI typecheck, build, and tests), plus release, nightly, site, Miri, native-library, and installer smoke workflows. `.config/`: `nextest.toml` and `hakari.toml`. `.githooks/`: pre-commit and pre-push. `.cursor/rules/`: `workshop-architecture.mdc` and `workshop-spa.mdc`.
  - Not tracked: `local/`, `target/`, `target-msrv/`, and `cabinet/`, which holds only an empty `_trash/`. `master` is at `83d40b1b9` (`Close plan: parallel-tool-calls`), three commits ahead of `origin/master`, with a clean worktree.
- Component boundaries:
  - Rust, enforced by `cargo test -p build-xtask` across normal, dev, and build dependencies: the Engine (`runvil*`) depends on no gateway, Workshop, Harness, or Plugin crate, and outside crates reach it only through `runvil` and `runvil-plugin`. The Harness chain runs `harness-gateway-client` on `harness` on `harness-runner` on the Engine pair. Plugin crates depend only on `runvil-plugin`, `shared-*`, `workspace-hack`, and outside libraries. The gateway depends on no Engine, Workshop, Harness, or Plugin crate and is reached only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop is a Host. Its Rust tiers depend one way: the server on features (`agents`, `workspace`, `user-state`, `run-log`), features on services (`menu`, `gateway`, `status`), and services on vocabulary (`protocol`, `registry`, `support`). The desktop app names only `workshop-server-api`, which names only `workshop-server`.
  - Workshop UI layers: `base/`, `services/`, and `parts/` may all import `@workshop/platform`, which never imports the app. Services own no views; parts are feature directories whose panels extend `WorkshopPart`. A lazily loaded part has an `index.ts` chunk entry and never imports the eager entry bundle (`main.ts` and the `*.contribution.ts` modules). `look/test/boundary.mjs` and `platform/test/boundary.mjs` guard the shared packages.
  - The agent panel's path: `services/agent-socket.ts` is the wire the service consumes, `services/agent-session.ts` (`AgentSessionService`) folds frames into transcript items, the pinned wait token, and `generating`, and `parts/agent/agent-session-view.ts` with `parts/chatbox/chat-box.ts` render from those. The tests drive the service through a scripted wire with the same surface.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized with one meaning each, and Engine crates say "the caller". `docs-claims.mjs` enforces the wording in every `AGENTS.md`, every `## Invariants` crate doc, and every `.cursor/rules` file.
  - The Workshop UI copies Cursor's IDE for look, layout, wording, and behavior (`crates/workshop/ui/AGENTS.md`). Files and directories are kebab-case, CSS sits beside its TypeScript with the `.ws-` class prefix and only token values, every persisted value goes through the `ui-storage` adapter to the server, and panels, menus, commands, services, and keybindings self-register through the `@workshop/platform` registries.
  - Error and status messages are written for a model to read: concise and naming what was required and what was found. Comments explain only non-obvious constraints, and every workaround cites its upstream issue URL.
  - Behavior changes ship with tests in the same change, and refactors preserve existing behavior tests.
  - Rust: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, and suppressions use `#[expect(..., reason = "...")]`. Every crate build script (51 of them) calls `build_ceiling::check()`, which fails any Rust file over 500 lines. 25 crate `lib.rs` files open with a `## Invariants` doc section. Source directories stay flat until a group reaches three files, with kebab siblings wired by `#[path]` below that.
  - Commits have a one-line summary, a prose body, a bullet list of changes, optional `Design:` lines, and a `Plan: vibe/<file>` trailer. A plan's closing commit is titled `Close plan: <words>` and holds only the trailer.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Track open waits oldest first and gate the generating flip [completed]

- Component: none
- Covers: D1-1 and C1-1, both cleanup goals, the whole Technical Design, and the whole Testing Plan. D1-1's "no wait open" check reads the open-wait list that C1-1 introduces, so both defects land in this one step and one commit.
- Starting point: `master` at `83d40b1b9` with a clean worktree. Run commands from `crates/workshop/ui` unless a command says otherwise.
- Tests first, in `crates/workshop/ui/test/agent-session-service.mjs`. Write each case as a scoped `{ }` block under a `// --- Title ---` banner that builds its own `makeWire()` and `AgentSessionService`, disposes them, and asserts through the file's `check(name, condition)` with lowercase behavior names:
  - Beside the model-ask block (lines 490-506), the D1-1 case: `tool_call` for `[c1 user-input/ask, c2 web/fetch]`, then `inputRequired("tok1")`, then `tool_call_update` for `c2`. Check `generating === false` and `pendingInputToken === "tok1"`, then check that `respond` turns `generating` on.
  - Beside the pin tests (lines 266-294), three C1-1 cases:
    - Two open waits pin the oldest and advance: `inputRequired("tok1")` then `inputRequired("tok2")` leaves `pendingInputToken === "tok1"`. Responding pins `tok2` with `generating` still off, and responding again leaves null with `generating` on.
    - Cancelling a wait removes it wherever it sits: with `tok1` and `tok2` open, `inputCancelled("tok1")` leaves `tok2`, as the Testing Plan says. In a second fixture in the same block, `inputCancelled("tok2")` leaves `tok1`. The second assertion is what makes this block fail at `83d40b1b9`, because today's handler already pins `tok2` by overwrite, so the first assertion passes there.
    - A reattach pins the oldest unresolved wait: `tok1`, `tok2`, the attach acknowledgement (a session frame with the same session id), then `inputRequired("tok1")` and `inputRequired("tok2")` again leave `tok1`. Prove there is no duplicate token by responding twice: the first respond pins `tok2`, the second leaves null.
- Tests first, in `crates/workshop/ui/test/agent-session-view.mjs`, beside the Stop tests (lines 1060-1085): drive the D1-1 sequence and check the composer's `data-action` stays `send`.
- Confirm the failures before touching source: run `node test/agent-session-service.mjs` and `node test/agent-session-view.mjs`. Every new block must report at least one failing check, and every existing check must still pass. Stop and re-plan if a new block passes or an existing check fails.
- Source, in `crates/workshop/ui/src/services/agent-session.ts`:
  - Add a private list of open wait tokens in announcement order as the source of truth for the pin. `pendingInputToken` returns the oldest open token, or null when none is open, which matches its doc.
  - The `wire.onInputRequired` handler (lines 259-270): append the token unless it is already open, keep closing thinking, clearing `stopped`, and turning `generating` off, then pin the oldest open token.
  - `respond` (lines 340-355): on a successful send, remove the spent token, pin the next oldest, and call `setGenerating(true)` only when no wait remains open. A failed send keeps every token and folds the connection error, as today.
  - The `wire.onInputCancelled` handler (lines 273-278): remove the cancelled token wherever it sits, then pin the oldest remaining token or null.
  - `acknowledge` (lines 386-400): clear every open token where it now calls `setPinnedToken(null)`, so the server's `resend_unresolved` rebuilds the list in creation order.
  - The `tool_call_update` case in `foldEvent` (lines 456-464): turn `generating` on only when `answersModelCall(toolCallId)` holds and no wait is open.
  - `setPinnedToken` (lines 633-639) stays the only place that publishes the pin, always fed from the oldest open token, so the view's subscription is unchanged.
  - Update the comments that describe a single pin - on the `onInputCancelled` handler, `respond`, `acknowledge`, and the `tool_call_update` case - so they describe the open-wait list and the gate.
- Leave unchanged: `agent-session-view.ts`, `chat-box.ts`, the Workshop server and its frames, the Engine, the Harness, and `../runvil-docs`. Stop while a question is open keeps today's meaning (D1-12). D1-2 and D1-17 stay deferred.
- Verify: rerun the two focused files. Then in `crates/workshop` run `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present`, and from the repository root run `node --test crates/workshop/ui/test/docs-claims.mjs`. All new checks pass, and the existing checks named in the Testing Plan ("input_required pins", "a spent token unpins", "input_cancelled unpins", "an open wait keeps generating off", and the view's Stop tests at lines 1303-1342) pass without edits.
- Setup edit to four existing checks in the model-ask block of `agent-session-service.mjs` (lines 490-525): "a tool_call_update answering a model batch call turns generating on", "a result for the current turn's call turns generating on", "an answered call turns it back on", and "a same-session acknowledgment leaves generating alone". Clear `tok1` before `c1`'s `tool_call_update`, and `tok2` before the next turn's `user_message`, in a way that leaves `generating` off (for example `inputCancelled`, not `respond`, since `respond` already turns `generating` on and would make the checks vacuous). Keep every assertion as it is, and confirm each still fails if the `tool_call_update` case stops turning `generating` on.
- Commit: one commit holding the source change and both test files, in the repository format: a one-line summary, a prose body, a bullet list of changes, `Design:` lines for the two reversible decisions (oldest-first open waits, and the generating flip gated on no open wait), and a `Plan: vibe/2026-10-11-4-workshop-waits.md` trailer. Nothing is pushed.

</step-1>

</execution-plan>
