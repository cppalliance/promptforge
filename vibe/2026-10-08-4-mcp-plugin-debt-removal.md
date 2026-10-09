---
name: MCP Plugin Debt Removal
overview: "Removal plan from the Debt Collector run on promptforge2 commits 20aace8 through 0202927 (the mcp-client-plugin plan). Three findings kept: one stale Invariants list (doc edit), one unused dependency (already removed on HEAD, verify only), and one hand-kept runtime service pairing (user chose option A: keep the design, add one behavior test)."
todos:
  - id: w1-invariants-list
    content: "D1-1: edit the Tier bullet in crates/workshop/server/src/lib.rs to name plugin-mcp and promptforge-plugin; run docs-claims (npm test in crates/workshop/ui) and cargo test -p build-xtask"
    status: pending
  - id: w2-serde-verify
    content: "D1-2: verify serde is absent from plugin-mcp's Cargo.toml and Cargo.lock entry (already removed on HEAD by a1201da73); change nothing if so"
    status: pending
  - id: w3-runtime-leg-test
    content: "D1-3 option A: add the remote-entry behavior test and header clause in crates/workshop/server/src/agents/tests.rs"
    status: pending
  - id: w3-fault-injection
    content: "D1-3: inject the wrong reader literal in plugin-mcp lib.rs line 91, confirm the new test and plugin-mcp's integration tests fail, then restore the file and confirm git diff is clean of it"
    status: pending
  - id: exit-gates
    content: "Run exit checks from AGENTS.md Verification: formatter, Workshop clippy with warnings denied, Workshop nextest trio, cargo test -p build-xtask"
    status: pending
isProject: false
---

# Debt Removal: MCP Client Plugin (20aace8 through 0202927)

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `c:\Users\Vinnie\cursor\promptforge2`, branch `vibe2`.
  - Six commits, baseline `475b96214ad8f423b382e74e326ff6be84dedc72` (parent of `20aace8ee`), endpoint `02029270146d0d49bf788fe4b68d62c8bef2a6c6`. All six name the plan `vibe/2026-10-08-1-mcp-client-plugin.md`: `20aace8ee` (Plugin contract names the tokio runtime service), `e37308797` (`Plugin::ready` and the Harness readiness wait), `1004599c6` (the `plugin-mcp` crate), `c74899f70` (Workshop installs servers from a named `mcp.json`), `c9ed649bf` (remove the reserved Lua `mcp` request), `020292701` (close plan).
  - Disposition ref is the endpoint. The worktree is excluded. The repository `HEAD` is `3013586da`, which descends from the endpoint.
- Cleanup goals:
  - Make the Workshop server's written dependency list match its manifest (D1-1).
  - Confirm the unused `serde` dependency in `plugin-mcp` is gone (D1-2).
  - Pin, with one behavior test, the one unprotected leg of the hand-kept runtime service pairing (D1-3), with no public interface change.
- Non-goals:
  - No change to the Plugin contract, `plugin-web`, or `plugin-mcp` source.
  - No typed runtime key, spawn service, or shared crate (options B, C, D below, all declined by the user).
  - No new structural check, and no work on the exposed pre-existing debt listed below.
- Success criteria:
  - `crates/workshop/server/src/lib.rs` names `plugin-mcp` and `promptforge-plugin`.
  - The new Workshop test passes on the current tree and fails under the fault injection in the Testing Plan.
  - `git diff` against `HEAD` touches only `crates/workshop/server/src/lib.rs` and `crates/workshop/server/src/agents/tests.rs`.
  - All exit gates pass.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1, introduced.
    - Commits: `20aace8ee` added the `promptforge-plugin` edge and `c74899f70` added the `plugin-mcp` edge to `crates/workshop/server/Cargo.toml`. Neither commit touches `crates/workshop/server/src/lib.rs`, and the design record's step file lists never name it.
    - Evidence: the `## Invariants` Tier bullet (lines 28 to 35) says the server may depend on "the Plugin crates `plugin-web` and `plugin-user-input`, and the Engine's public API `promptforge`". The manifest at the endpoint also depends on `plugin-mcp` and `promptforge-plugin`.
    - Impact: no runtime effect. The stated boundary is false in the one place it is written (`crates/workshop/server/AGENTS.md` holds no list), and the crate docs tell readers to consult it before adding an import. `cargo test -p build-xtask` does not compare the prose with the manifest, so no gate objects.
    - Reversal cost: one doc edit in one file.
    - Target state: still stale at the endpoint and at `HEAD`.
  - D1-3, introduced, prospective debt.
    - Commits: `20aace8ee` replaced `plugin-web`'s typed public `TOKIO_RUNTIME: ServiceKey<Handle>` with the bare name `pub const TOKIO_RUNTIME: &str` in `crates/promptforge-plugin/src/service.rs` (line 130). `1004599c6` added the second production reader.
    - Mechanism: three production sites each build `ServiceKey::<Handle>::new(...)` from the name by hand: `crates/plugin-web/src/web.rs` line 40, `crates/plugin-mcp/src/lib.rs` line 91, and `crates/workshop/server/src/agents.rs` lines 74 to 75. Nothing checks that the types agree. A provider of another type reads as missing.
    - Reversal burden: a typed replacement changes a public interface or a dependency direction and edits `promptforge-plugin`, `plugin-web`, `plugin-mcp`, and the Workshop server together.
    - Impact: low. The decision record chose this deliberately because the contract is an Engine crate and cannot depend on `tokio`, and it lists revisit conditions. No behavior is wrong today.
    - Target state: unchanged at the endpoint and at `HEAD`.
- Cheap fixes:
  - D1-2, cheap fix. `1004599c6` added `serde.workspace = true` to `crates/plugin-mcp/Cargo.toml` and nothing in the crate uses it. The design record's dependency budget listed it with no use, and no Deferred item reserves it. Already fixed on `HEAD` by `a1201da73` ("Remove the unused serde dependency from plugin-mcp"), which deletes the manifest line and the `Cargo.lock` entry. The disposition ref is the endpoint, where the debt exists.
- Exposed pre-existing debt (reported only, not in this plan unless the scope is widened):
  - The same Invariants list already omitted `gateway-api-discovery` and `shared-loopback` at the baseline.
  - No tracing subscriber is installed anywhere under the Workshop crates, so every `tracing::warn!` in the server, including the new bad-`mcp.json` warnings, has no output in this tree.
  - A slot naming a tool of a ready server that lists zero tools reports a missing Plugin (`crates/promptforge-internal/engine/src/execute/fill.rs` lines 42 to 50). The design record already defers it.
- Rejected candidates: 25 in total. 10 residual-but-acceptable, 6 weak/speculative, 7 false, 2 unrelated pre-existing.
  - Residual-but-acceptable: the `Plugin::ready` wait with no Harness timeout (the bound sits in each Plugin and `plugin-mcp` enforces two minutes); `prepare` kept beside `prepare_noting_cancel` (17 test call sites, public in a crate whose budget is zero); empty `tools()` until `ready()` resolves (one sequenced caller); `construct` reading the runtime from `HostServices` (signature fixed by the contract); Workshop logging and skipping a bad `mcp.json`; the explicit local-server deferral; the pre-implementation cancel rule in the closed plan; a relative `[agents] mcp` path; the new `AgentsConfig::mcp` setting; no reconnect after a server fails.
  - False: `plugin-mcp`'s `PACKAGE` as new surface (only public item); the Workshop server's direct edge to `promptforge-plugin` (xtask allows it and the facade has no re-export of `ServiceKey`); the redirect-refusal claim (checked in rmcp 3.5.1); `isError` text reaching the model unwrapped (the Engine nonce-wraps it); tool name collisions (the Engine drops the later tool with a warning); `serde_json` `preserve_order` (off in the lock and guarded by a test); the `rmcp` exact pin.
  - Weak/speculative: removal of the reserved Lua `mcp` request; thin handshake failure reasons (deliberate, to keep URLs out); `AgentSessions::new` building before the omit seam; entry parsing edge cases; unwrapped tool descriptions and schemas from servers; rmcp's own error logs naming a URL.
  - Unrelated pre-existing: the zero-tool fill wording and the missing Workshop tracing subscriber (both listed above).

</product-contract>
<implementation-contract>

## Technical Design

- D1-1, `crates/workshop/server/src/lib.rs`: edit only the Tier bullet under `## Invariants`, lines 28 to 35. The Plugin and Engine clauses become:

```text
//!   the Harness's public crates `harness` and `harness-gateway-client`,
//!   the Plugin crates `plugin-web`, `plugin-user-input`, and
//!   `plugin-mcp`, and the Engine's public crates `promptforge` and
//!   `promptforge-plugin`.
```

  - Keep Engine, Harness, and Plugin capitalized and prefix the rest of the bullet as it stands. Do not touch the two older omissions (`gateway-api-discovery`, `shared-loopback`). `crates/build-xtask/src/product.rs` (`PUBLIC_PROMPTFORGE`, around line 144) already names `promptforge` and `promptforge-plugin` as the public Engine pair, so the wording matches the boundary rule.
- D1-2: no change. At `HEAD`, line 19 of `crates/plugin-mcp/Cargo.toml` is `serde_json.workspace = true` and the `Cargo.lock` entry for `plugin-mcp` lists `axum`, `promptforge-plugin`, `reqwest`, `rmcp`, `serde_json`, `tokio`, `tracing`, `workspace-hack` and no `serde`.
- D1-3, `crates/workshop/server/src/agents/tests.rs`: add one `#[tokio::test]` beside `the_host_context_installs_the_servers_of_the_named_file`, and one clause to the file's top doc comment (lines 1 to 8) saying a remote `mcp.json` entry reads the runtime the server provides.
  - Reuse the file's existing helpers: `requiring`, `run_prompt`, `with_plugins`, `services`, `Registry`. Name it in the file's sentence style, for example `a_remote_server_installs_on_the_runtime_the_server_provides_and_fails_only_on_its_connection`.
  - Get a refused address the way `crates/plugin-mcp/src/connect-tests.rs` line 58 does: bind a listener on `127.0.0.1:0`, read its port, drop it. Use `std::net::TcpListener` so the server crate needs no new tokio feature.
  - Build the host with `with_plugins(services(&Registry::new()), vec![("docs".to_owned(), serde_json::json!({ "url": format!("http://127.0.0.1:{port}/mcp") }))])`, run `requiring("docs")` through `run_prompt`, and match `Some(RunOutcome::Failed { message, .. })`.
  - Assert the message contains `- docs is unavailable: the MCP handshake failed` and does not contain `promptforge/tokio-runtime`. The first proves `construct` passed the runtime lookup through the Workshop provider and ran its connection task. The second proves the failure was the connection and not the missing-service reason (`crates/plugin-mcp/src/lib.rs` lines 107 to 109). The ready-failure format `- <plugin> is unavailable: <reason>` is the one `crates/harness-internal/runner/tests/it/prepare-ready.rs` line 282 asserts.
  - Lifecycle and failure: the connection task is already settled when the run ends, and dropping the host aborts any leftover task (`Server::drop`). Nothing is left running. If the run does not settle inside `run_prompt`'s 10 second limit, find out why before changing the limit.
- Data flow check: the provider side is `services()` in `crates/workshop/server/src/agents.rs` (lines 187 to 198), which provides the runtime under `RUNTIME` built from `promptforge_plugin::TOKIO_RUNTIME`. The reader side is `plugin-mcp`'s own `RUNTIME`. The test drives both through the production `with_plugins` and `services`, which is the one leg no existing test covers: Workshop's current MCP tests install only refused local entries.

</implementation-contract>
<verification-contract>

## Testing Plan

- D1-3 focused:
  - Run the new test through the Workshop-suite nextest command in `AGENTS.md` Verification, filtered to its name. It is expected to pass on the current tree. D1-3 is prospective debt, not incorrect behavior, so this is a protection and not a regression test. A pass does not prove the finding false.
  - Fault injection to show it can fail: temporarily change the reader key in `crates/plugin-mcp/src/lib.rs` line 91 to `ServiceKey::new("promptforge/tokio-runtime-x")`. Run the new test and confirm it fails on both assertions. Run `plugin-mcp`'s integration tests and confirm they fail too, which shows the existing net. Then restore the file and confirm `git diff` no longer lists it. Never commit the injected change.
- Existing protections reused, none replaced:
  - `crates/plugin-mcp/tests/it/main.rs` lines 22 to 31 (independent `ServiceKey<Handle>` provider) and 216 to 227 (`a_missing_runtime_service_fails_construct_naming_it`).
  - `crates/plugin-web/src/web-tests.rs` lines 23 and 105.
  - Workshop `a_prompt_declaring_web_prepares_on_the_servers_installed_plugins` (`crates/workshop/server/src/agents/tests.rs` lines 126 to 136).
  - The contract's `crates/promptforge-plugin/src/service-tests.rs` (a provider of the wrong type reads as missing).
- D1-1: run `npm test` in `crates/workshop/ui`, which runs `test/docs-claims.mjs` over every `## Invariants` doc (build the UI first if its boot tests need the bundle). Run `cargo test -p build-xtask` for the Invariants marker and boundary rules. Compare the new list with the dependency table of `crates/workshop/server/Cargo.toml` by eye.
- D1-2: confirm by search that `crates/plugin-mcp/Cargo.toml` has no `serde.workspace` line and the `plugin-mcp` entry in `Cargo.lock` lists no `serde`. Both held when this plan was written.
- Exit checks, all from `AGENTS.md` Verification:
  - the formatter check;
  - Workshop clippy with warnings denied;
  - the Workshop nextest trio (`workshop`, `workshop-server`, `workshop-server-api`);
  - `cargo test -p build-xtask`.
  - The rustdoc gate excludes the Workshop crates, so D1-1 needs no rustdoc run.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - D1-1 is a prose edit, not a new xtask check. No check compares the list with the manifest and one stale line does not justify one. Consequence: the list can drift again, as it did for `gateway-api-discovery` and `shared-loopback`. Confidence: high - the edit is two phrases and fixes the sentence the target made false.
  - D1-3 is one behavior test through the production install path, not a structural check. A refused local port stands in for a server because `plugin-mcp`'s fixture lives in another crate's tests and the refusal path alone proves the runtime lookup. Confidence: medium - the failure text format is read from other tests, not run.
  - D1-2 closes by verification because `HEAD` already holds the removal.
- User-resolved architecture choices: D1-3 option A. Keep the recorded design (the contract owns the runtime service's name as a string, each tokio-based Plugin and the Host build their typed key from it) and add the one behavior test.
- Rejected alternatives, assumptions, and risks:
  - B, record only: cheapest, but the Workshop-to-`plugin-mcp` leg stays unpinned.
  - C, a runtime-agnostic spawn service in the contract: removes the string at its source. It changes the contract's public interface, makes `plugin-web` rebuild its abort-on-drop fetches on it, and still needs a tokio runtime underneath. The design record rejected it (line 340).
  - D, a new shared crate of tokio-typed keys: it changes component ownership and dependency direction and adds an allowed crate to the Plugin boundary rule in `build-xtask`. The design record rejected it (line 342).
  - A typed `ServiceKey<tokio::runtime::Handle>` in the contract, directly or behind a feature: not viable while the Engine manifest guard forbids `tokio` there (record line 341).
  - Assumption: a refused connection settles quickly into a handshake failure (proved for the crate by `connect-tests.rs` line 58). Risk: Workshop's Harness could word the refusal differently. If so, keep both halves of the assertion (names the handshake failure, does not name the runtime service) and adjust only the text.
  - Risk: another process could take the port between the drop and the connect. `connect-tests.rs` accepts the same race.
  - Judgment call: the challenger and the analysis disagreed on D1-3. The rule for prospective debt needs a mechanism and a present reversal burden, both visible in the tree, and no runtime consequence. If a reader holds that it also needs a consequence, D1-3 drops to rejected, and the test is still cheap and worth keeping.

### Deferred and Out of Scope

- Replacing the string name with a typed key: revisit when a Host runs on a runtime other than tokio, when a Plugin must link a different tokio than the Host, or when the contract grows several runtime-typed services (design record lines 340 to 342).
- The `gateway-api-discovery` and `shared-loopback` omissions in the server's Invariants list: exposed pre-existing debt. Revisit when the user widens the scope, which would add two phrases to the same D1-1 edit.
- No tracing subscriber under the Workshop crates: revisit when one is added or with the Workshop server status UI (record line 403).
- A ready server listing zero tools reported as a missing Plugin: revisit when a configured server lists zero tools (record line 406).
- Server-written tool descriptions and schemas reaching the model unwrapped (bound or wrap them in `crates/plugin-mcp/src/server.rs` `catalog`): revisit when the design record decides whether descriptor text is trusted, or when an untrusted server is configured.
- rmcp's own error logs can name a request URL that holds an expanded variable: revisit when a tracing subscriber is added in Workshop.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace `default-members` is the gateway only); a headless gateway is `cargo build --locked -p gateway --no-default-features`; the desktop app is `cargo build -p workshop`; any one crate is `cargo build --locked -p <crate>`. The Workshop UI builds with `npm run build --workspace ui` inside `crates/workshop` (esbuild), after `npm ci --prefix crates/workshop`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>`. CI also runs single named tests as `cargo test --locked -p <crate> <test-name>`.
- Component test command pattern: `cargo nextest run --locked -p <crate>` runs a crate's unit and integration tests. One integration target is `cargo nextest run --locked -p <crate> --test <target>`, where the target is `it` for `plugin-mcp`, `promptforge-plugin`, and `gateway`, and `suite` for `harness`. A crate's unit tests only: `cargo nextest run --locked -p <crate> --lib`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then the Workshop crates separately with `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. Structural and boundary checks are `cargo test -p build-xtask` (CI adds `cargo nextest run --locked -p build-xtask --run-ignored only`), and the JS suites are `npm test --workspaces --if-present` under `crates/workshop` and `npm test` under `crates/gateway/config-ui/ui`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and for Workshop `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. The gate sets `CARGO_BUILD_WARNINGS=deny`. The headless build-shape gate is `cargo check -p gateway --no-default-features`; no other standalone `cargo check`. TypeScript is checked with `npm run typecheck`. Workspace lints (`[workspace.lints]` in the root `Cargo.toml`) deny `clippy::all` and `pedantic`, `unwrap_used`, `expect_used`, and `allow_attributes`; a crate inherits them with `[lints] workspace = true`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; the `.githooks/pre-commit` hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, and for the facade `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`. The facade surface check is `cargo +<pinned nightly> xtask api --check`.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file named `<stem>-tests.rs` and are wired from `<stem>.rs` with `#[cfg(test)]`, `#[path = "<stem>-tests.rs"]`, and `mod tests;`. Examples: `crates/plugin-mcp/src/connect-tests.rs`, `entry-tests.rs`, `result-tests.rs`, `server-tests.rs`, and `crates/promptforge-plugin/src/context-tests.rs`. Larger groups use a `tests/` subdirectory beside the module, such as `crates/plugin-web/src/fetch/tests.rs`.
  - Integration tests live in `crates/<crate>/tests/<it|suite>/main.rs` with sibling modules such as `fixture.rs`, so each crate has one integration binary. `harness = false` appears only in the `lua` and `engine` crates.
  - Plugin tests lend a Plugin its context through `promptforge_plugin::testing::TestCall`, enabled by the dev-dependency feature `test-support`.
  - Test names are full descriptive sentences in snake case, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. Test code may use `expect`, and a helper module that does says so with `#![expect(clippy::expect_used, reason = "...")]`.
  - No doctests: `build-xtask` fails any compiled code block in a doc comment, so examples go in `text`, `json`, or `toml` fences.
  - JS tests are `*.test.mjs` run with `node --test`. Slow-test and heavy-suite limits are in `.config/nextest.toml`.
- Directory map:
  - `crates/` holds every Rust crate and the TypeScript packages: flat crates `promptforge` (Engine facade), `promptforge-plugin` (Plugin contract), `plugin-mcp`, `plugin-web`, `plugin-user-input` (Plugins), `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `workspace-hack` (hakari), and the `build-*` tooling crates (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`).
  - Manifestless containers hold each family's private crates: `crates/promptforge-internal/` (types, parser, lua, vfs, model-client, engine), `crates/harness-internal/runner`, `crates/gateway/` (app, config, config-ui, local, logging, progress, protocol, routing, cloud-providers, web-search, and `stt/` with api, engine, backend-whisper, whisper-ffi), and `crates/workshop/` (desktop, server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace, run-log, agents, plus the TypeScript packages `ui`, `look`, `platform`). `crates/shared-ui` is a TypeScript and CSS package, not a crate.
  - `guide/` holds the user documentation sources (`books`, `chrome`, `landing`); `prompts/` holds example prompts; `tools/` holds `.mjs` sidecar and live-test scripts; `vibe/` holds plan files; `images/` holds README art.
  - `.github/workflows/` holds CI and release workflows, `.githooks/` holds `pre-commit` and `pre-push`, `.config/` holds `nextest.toml` and `hakari.toml`, `.cursor/rules/` holds two Workshop rules, and `.cargo/config.toml` defines the `xtask` and `workshop` aliases.
- Component boundaries (normal dependency directions, read from `cargo metadata`):
  - Engine: `promptforge` depends on `promptforge-engine`, `-lua`, `-parser`, `-model-client`, `-types`, and `-vfs`; `-types` and `-vfs` depend on nothing in the workspace, and `-model-client`, `-lua`, `-parser`, and `-engine` stack on them.
  - Plugin contract: `promptforge-plugin` depends only on `promptforge-types` and `promptforge-vfs`. `plugin-mcp`, `plugin-web`, and `plugin-user-input` each depend only on `promptforge-plugin`, plus `workspace-hack`, and never on each other or the Harness.
  - Harness: `harness` depends on `harness-runner`, `promptforge`, and `promptforge-plugin`; `harness-gateway-client` depends on `harness`, `plugin-web`, and `promptforge`.
  - Gateway: `gateway` (the app) depends on its family crates and `shared-loopback`; `gateway-api-types` is the leaf that the family and `workshop-gateway` share; `gateway-api-discovery` is also used by `workshop`.
  - Workshop (a Host): `workshop-server` is the top, depending on the Harness, the three Plugins, `promptforge`, and the `workshop-*` crates; `workshop-server-api` depends on `workshop-server`, and `workshop` depends on `workshop-server-api`. `workshop-protocol`, `-support`, and `-registry` are the base tier.
  - `cargo test -p build-xtask` enforces the product and container boundaries, the Plugin family's allow-list, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain, `unsafe_code = "deny"`, and every crate inherits the workspace lints; `missing_docs` and `unreachable_pub` warn, so crates expose a minimal public surface (`plugin-mcp` exposes only `PACKAGE`).
  - Every crate's `lib.rs` carries crate-level `//!` docs ending in a `## Invariants` list; the four defined terms Engine, Harness, Host, and Plugin are capitalized and mean one thing, and `AGENTS.md` and `.cursor/rules` are scanned for them.
  - Source directories are flat: a subdirectory needs at least three files, otherwise a sibling `foo-bar.rs` is wired with `#[path = "foo-bar.rs"] mod bar;`.
  - Dependencies are declared once in `[workspace.dependencies]` with a comment saying why a pin or feature set exists, and a crate uses `workspace = true` plus `workspace-hack` (cargo-hakari).
  - Comments state a non-obvious constraint or cite an upstream issue URL for a workaround; error messages are concise and self-contained for model consumption.
  - Behavior changes ship with tests in the same change. Prefer types and compiler checks, then behavior tests.
  - CI runs fmt, clippy, nextest, docs, `cargo deny`, `cargo audit`, `cargo hakari verify`, and the `xtask api` surface check; the pre-push hook runs the headless check, clippy, and `cargo deny`.

</project-survey>
<execution-plan>

## Execution Instructions

Run the steps in order. The two steps touch different files, but Step 2 closes the plan with the exit checks, so it goes last. Commit messages follow the repository's recent history (plan trailers and wording style from `git log`). Never commit the fault injection in Step 2.

<step-1>

### Step 1: Fix the Workshop server Invariants list and confirm the serde removal [completed]

- Component: none
- Covers: D1-1 and D1-2 (work items W1 and W2).
- Artifact: `crates/workshop/server/src/lib.rs`, the Tier bullet under `## Invariants` (lines 28 to 35). Replace the Plugin and Engine clauses with the text in Technical Design: Plugin crates `plugin-web`, `plugin-user-input`, and `plugin-mcp`, and Engine public crates `promptforge` and `promptforge-plugin`. Keep Engine, Harness, and Plugin capitalized. Leave the rest of the bullet as it is and do not touch the older `gateway-api-discovery` and `shared-loopback` omissions.
- Verify D1-1:
  - Run `npm test` in `crates/workshop/ui` (build the UI first if its boot tests need the bundle). This runs `test/docs-claims.mjs` over every `## Invariants` doc.
  - Run `cargo test -p build-xtask` for the Invariants marker and boundary rules.
  - Compare the new list with the dependency table in `crates/workshop/server/Cargo.toml` by eye.
- Verify D1-2 (no edit expected): search `crates/plugin-mcp/Cargo.toml` for a `serde.workspace` line and the `plugin-mcp` entry in `Cargo.lock` for `serde`. On `HEAD` both are absent because `a1201da73` removed them, so change nothing. Only on a branch that lacks `a1201da73`: delete the manifest line and the lock entry, then build `plugin-mcp` and the Workshop crates, and include those two files in this step's commit.
- Commit: one commit holding only the `lib.rs` edit (plus the two serde files in the branch-lacks-`a1201da73` case). There is no new test because the change is prose and the existing docs-claims and xtask checks are the gates.

</step-1>

<step-2>

### Step 2: Add the remote-server runtime test, prove it can fail, and run the exit checks

- Component: none
- Covers: D1-3 option A and the exit checks (work items W3 and Exit).
- Artifact: `crates/workshop/server/src/agents/tests.rs`.
  - Add one clause to the top doc comment (lines 1 to 8) saying a remote `mcp.json` entry reads the runtime the server provides.
  - Add one `#[tokio::test]` beside `the_host_context_installs_the_servers_of_the_named_file`. Suggested name: `a_remote_server_installs_on_the_runtime_the_server_provides_and_fails_only_on_its_connection`.
  - Reuse the file's helpers `requiring`, `run_prompt`, `with_plugins`, `services`, and `Registry`.
  - Get a refused address with `std::net::TcpListener`: bind `127.0.0.1:0`, read the port, drop the listener. This needs no new tokio feature.
  - Build the host with `with_plugins(services(&Registry::new()), vec![("docs".to_owned(), serde_json::json!({ "url": format!("http://127.0.0.1:{port}/mcp") }))])`. Run `requiring("docs")` through `run_prompt` and match `Some(RunOutcome::Failed { message, .. })`.
  - Assert the message contains `- docs is unavailable: the MCP handshake failed` and does not contain `promptforge/tokio-runtime`.
  - If the run does not settle inside `run_prompt`'s 10 second limit, find out why before changing the limit. If Workshop's Harness words the refusal differently, keep both halves of the assertion and adjust only the text.
- Verify focused: run the new test with the Workshop nextest command (`cargo nextest run --locked -p workshop-server <test-name-substring>`). It is expected to pass on the current tree.
- Fault injection (temporary, never committed):
  - In `crates/plugin-mcp/src/lib.rs` line 91, change the reader key to `ServiceKey::new("promptforge/tokio-runtime-x")`.
  - Confirm the new test fails on both assertions. Run `plugin-mcp`'s integration tests (`cargo nextest run --locked -p plugin-mcp --test it`) and confirm they fail too.
  - Restore `crates/plugin-mcp/src/lib.rs` and confirm `git diff` no longer lists it.
- Exit checks, run once with Step 1 already committed and before committing this step:
  - `cargo fmt --all --check`
  - Workshop clippy with warnings denied: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` with `CARGO_BUILD_WARNINGS=deny`
  - the Workshop nextest trio: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo test -p build-xtask`
  - No rustdoc run is needed because the rustdoc gate excludes the Workshop crates.
  - Fix any finding in the file that owns it. Step 1 is already committed and its commit is never rewritten, so a finding in `lib.rs` is fixed in this step's commit.
- Final check: `git diff` against `HEAD` before the commit lists only `crates/workshop/server/src/agents/tests.rs`, plus `crates/workshop/server/src/lib.rs` when an exit check forced a fix there. Across both commits the plan touches only `crates/workshop/server/src/lib.rs` and `crates/workshop/server/src/agents/tests.rs`.
- Commit: one commit holding the test and the header clause together.

</step-2>

</execution-plan>