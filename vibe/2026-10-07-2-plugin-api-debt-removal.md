---
name: Plugin API debt removal
overview: "Remove the debt the plugin-api branch (3e917d209..e7c1c7733) added, plus two pre-existing items the operator brought into scope: make the public `Package` label non-exhaustive with a const constructor, delete the dead `ToolView` binding methods, and remove the unused `ToolDescriptor::wire_name` field."
todos:
  - id: package-const-ctor
    content: "D1-1: make Package #[non_exhaustive] with const fn new/prelude/needs, migrate the 20 literal sites and docs, add the package test"
    status: pending
  - id: toolview-dead-methods
    content: "D1-2 and C-1: delete ToolView::binding and ToolView::offered_binding and the two test assertions"
    status: pending
  - id: remove-wire-name
    content: "D1-22: remove ToolDescriptor::wire_name, InvalidWireName, validate_identifier, TestTool::wire_name, the host-run wire step; update all ToolDescriptor::new calls and tests; regenerate public-api.txt"
    status: pending
  - id: exit-gates
    content: Run every Exit criteria command in the Testing Plan
    status: pending
isProject: false
---

# Plugin API debt removal

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `promptforge2` at `c:\Users\Vinnie\cursor\promptforge2`. Work lands on branch `plugin-api`, on top of `e7c1c7733` ("Close plan: plugin-api-minimal").
  - Target work audited: `3e917d209..e7c1c7733`, the eight commits of the plan record `vibe/2026-10-07-1-plugin-api-minimal.md` (`219410b46`, `73e438fe9`, `1971cd166`, `30f70d2e9`, `7c8302d21`, `a1b9bfaec`, `ae8cf7f0e`, `e7c1c7733`). The baseline `3e917d209` is the merge base of `upstream/master` and `plugin-api`.
  - Every path below is relative to the repository root.
- Cleanup goals:
  - A new field on `Package` never again breaks every Plugin crate (D1-1).
  - No dead method remains on the internal `ToolView` trait (D1-2, C-1).
  - `ToolDescriptor` carries no field that nothing reads, and no Plugin tool is dropped because of that field's value (D1-22).
- Non-goals:
  - No change to Plugin behavior, tool dispatch, the offering, stop and cancel, or any event or persisted format.
  - No public API listing for `promptforge-plugin` (already deferred by the plan record).
  - No edits to the historical plan record `vibe/2026-10-07-1-plugin-api-minimal.md`.
- Success criteria:
  - Every check in the Testing Plan passes, including every Exit criteria command.
  - `Package` is `#[non_exhaustive]`, and every Plugin crate and fixture builds its label with `Package::new(..)` plus setters.
  - `ToolView` has no `binding` or `offered_binding` method.
  - `ToolDescriptor` has no `wire_name`, `ToolDescriptor::new` takes `(id, description, parameters_schema)`, and `crates/promptforge/public-api.txt` reflects that.

## Functional Specification

### Debt Inventory

- Debt added:
  - **D1-1 (introduced, `30f70d2e9` and `7c8302d21`): `Package` is an exhaustive public struct built by literal everywhere.**
    - Evidence: `crates/promptforge-plugin/src/plugin.rs` declares `#[derive(Clone, Copy)] pub struct Package` with four `pub` fields (`name`, `prelude`, `needs`, `construct`) and no `#[non_exhaustive]`. `promptforge-plugin` is a public entry crate (`PUBLIC_PROMPTFORGE` in `crates/build-xtask/src/product.rs`, `ENGINE_ROOT_CRATES` in `crates/build-xtask/src/engine_guards.rs`). The `30f70d2e9` message itself says a new field breaks every existing literal.
    - Sites: 20 struct-literal sites in 11 files. Production: `crates/plugin-web/src/web.rs`, `crates/plugin-user-input/src/lib.rs`. Tests: `crates/promptforge-plugin/tests/it/package.rs`, `crates/harness/tests/suite/asker.rs`, `crates/harness-internal/runner/tests/it/{asker,prepare-host-services,prepare-input,prepare,scripted,tool_context}.rs`, and ten in `crates/harness-internal/runner/src/host-tests.rs`.
    - Impact: any new label field (the deferred package pins, MCP packages, DLL adapter packages) breaks all of them, and adding `#[non_exhaustive]` later forbids the `const PACKAGE: Package = Package { .. }` idiom outside the crate, a second break. Every other new public type in the target (`InstallError`, `UnavailablePlugin`, `ToolCatalog`) is non-exhaustive.
    - Reversal cost: public interface of a public crate plus coordinated edits in `plugin-web` and `plugin-user-input`; lowest now, while every consumer is in this repository.
    - Target state: `#[non_exhaustive]` plus a `const fn` constructor and `const fn` setters; future fields become additive.
- Cheap fix:
  - **D1-2 (cheap fix, `a1b9bfaec`): `ToolView::offered_binding` is dead.** The trait method (`crates/promptforge-internal/lua/src/handles.rs`, trait `ToolView`) and its only impl (`impl ToolView for Mutex<ToolSet>`) are called only by two assertions in `the_view_snapshots_the_offering` (`crates/promptforge-internal/lua/src/handles-tests.rs`). Every production lookup uses the inherent `ToolSet::offered_binding` (`scheduler/tool_call.rs`, `lua/src/tools.rs`, `lua/src/vm/state.rs`). `ToolView` is internal (not in `public-api.txt`). The `dead_code` lint does not fire on `pub` trait methods, which is why it survived.
- Pre-existing debt the operator added to scope:
  - **C-1 (pre-existing): `ToolView::binding` has no caller at all**, not even a test, at baseline or endpoint. Same trait, same edit as D1-2.
  - **D1-22 (pre-existing): `ToolDescriptor::wire_name` is required but never advertised.** Models see slot aliases and, for offered tools, names derived from the id. Its only reader is `ToolCatalog::new` (`crates/promptforge-internal/types/src/tools/registry.rs`), whose check `HostRunContext`'s per-tool validation (`crates/harness-internal/runner/src/host-run.rs`) uses to drop a Plugin tool with an illegal value. The Lua tool handle computes its own `wire_name` from `id.name()` (`crates/promptforge-internal/lua/src/tools/userdata.rs`). No production code serializes `ToolDescriptor`.
- Rejected candidates (25):
  - 10 residual-but-acceptable: deliberate, recorded in the plan record's Decision Record or Deferred list, and failing closed (zero-tool Plugin reported missing, old-form prompts refused, old transcripts reframed, web fetch-policy code reserved by a `Deferred:` trailer, single-entry dispatch and typed service lookup, offered-name collisions, three ask-fixture copies forced by the Harness boundary, unused `contract.plugins` UI field that still validates, full-id script calls to undeclared Plugins, and the Engine's new `tracing` dependency, which the plan's "with a log line" requires).
  - 6 weak or speculative: no demonstrated consequence or an additive fix (nested-id dispatch, `Drop` timing wording, `ToolView` churn, per-call offering clone, structural size leads, a stale `RunServices` comment in an untouched file).
  - 7 false: the code refutes them (unreachable catalog fallback, `ToolContext` access borrowing, the `1971cd166` `Repairs:` claim, weakened assertions that no longer exist, Workshop install errors, model calls to unadvertised offered names, web cancellation).

</product-contract>
<implementation-contract>

## Technical Design

- **`Package` (D1-1), `crates/promptforge-plugin/src/plugin.rs`:**
  - Add `#[non_exhaustive]`. Keep `#[derive(Clone, Copy)]`, the four `pub` fields (the Host in `crates/harness-internal/runner/src/host.rs` reads them, which `#[non_exhaustive]` still allows), and `impl fmt::Debug for Package`.
  - Add, each `#[must_use]` and `const fn`:
    - `Package::new(name: &'static str, construct: <the existing construct fn type>) -> Package`, with `prelude: None` and `needs: &[]`.
    - `Package::prelude(self, prelude: &'static str) -> Package`, setting `Some(prelude)`.
    - `Package::needs(self, needs: &'static [ServiceId]) -> Package`.
  - Reuse the existing `#[expect(clippy::type_complexity, reason = ...)]` for the `construct` parameter rather than adding a public type alias.
  - Every label becomes `pub const PACKAGE: Package = Package::new("vendor/name", construct)` with `.prelude(PRELUDE)` and `.needs(NEEDS)` where they were non-default. `plugin-user-input` uses both setters; `plugin-web` uses neither.
  - Update the crate docs and doc comments that show the literal form (`crates/promptforge-plugin/src/{lib,plugin}.rs`, the Plugin crates' docs, and any `AGENTS.md` or `## Invariants` text found by the search in the Testing Plan).
- **`ToolView` (D1-2, C-1), `crates/promptforge-internal/lua/src/handles.rs`:**
  - Delete `fn binding` and `fn offered_binding` from `trait ToolView` and from `impl ToolView for Mutex<ToolSet>`.
  - Keep the inherent `ToolSet::binding` and `ToolSet::offered_binding`, and keep `ToolView::{bindings, always, offered}`, which `tool_set_snapshot` in `crates/promptforge-internal/engine/src/execute/context.rs` reads.
- **`ToolDescriptor::wire_name` (D1-22):**
  - `crates/promptforge-internal/types/src/tools/descriptor.rs`: remove the `wire_name` field and the `wire_name` parameter of `ToolDescriptor::new`, which becomes `new(id, description, parameters_schema)`. Update the type and module docs (`descriptor.rs`, `types/src/tools.rs`) that describe a wire name.
  - `crates/promptforge-internal/types/src/tools/registry.rs`: remove the wire-name check in `ToolCatalog::new`, `ToolCatalogError::InvalidWireName`, and `ToolCatalogErrorKind::InvalidWireName`. `ToolCatalog::new` then refuses only repeated ids. Delete `validate_identifier` in `types/src/tools/ids.rs`, whose only caller is that check.
  - `crates/harness-internal/runner/src/host-run.rs`: per-tool validation keeps containment (the tool sits under its Plugin's name) and the shared `seen` set (no repeated id); drop the per-tool `ToolCatalog::new(slice::from_ref(tool))` wire-name step and its doc and comment text.
  - `crates/promptforge-internal/engine/src/test_support/tools.rs`: remove `TestTool::wire_name`, its implementations, and the test double built only to carry a transport-illegal wire name, with the tests that used it.
  - Update every `ToolDescriptor::new(` call: about 33 calls in about 30 files, four in production (`crates/plugin-web/src/web.rs` twice, `crates/plugin-user-input/src/ask.rs`, `crates/promptforge-internal/engine/src/execute/bindings.rs`), the rest in tests and `crates/promptforge/examples/greeter.rs`.
  - Unchanged on purpose: the Lua tool handle's `wire_name` field (`lua/src/tools/userdata.rs`, derived from `id.name()` and readable by scripts), model-client tool-schema name validation (`model-client/src/detail.rs`, `client/wire.rs`), which validates aliases and offered names, and the `wire name` wording in `lua/src/protocol/request.rs`, the scripted-chat test support, and the gateway cloud providers, which mean other things.
  - Regenerate `crates/promptforge/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`.
- Data, persistence, and failure: no persisted or wire format changes. `ToolDescriptor` has no `deny_unknown_fields`, so JSON that still carries `wire_name` deserializes. No production code writes descriptors.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused:
  - D1-1: in `crates/promptforge-plugin/tests/it/package.rs`, add `a_package_from_new_has_no_prelude_and_no_needs_until_set`. It builds a `const` label with `Package::new(..)`, and another with `.prelude(..).needs(..)`, and asserts each field. It fails to compile before the change because `Package::new` doesn't exist. Run `cargo nextest run --locked -p promptforge-plugin --all-features`.
  - D1-1: every migrated site compiles. rustc's E0639 refuses any struct-literal `Package` left outside `promptforge-plugin`, so no extra guard is added.
  - D1-2 and C-1: `the_view_snapshots_the_offering` keeps its `ToolView::offered` assertion and drops the two `ToolView::offered_binding` assertions; `an_offered_binding_is_found_by_its_name_and_never_as_a_declared_slot` still covers the inherent method every production lookup uses. Run `cargo nextest run --locked -p promptforge-lua -p promptforge-engine --all-features`.
  - D1-22: in `crates/harness-internal/runner/src/host-tests.rs`, `begin_run_drops_a_tool_outside_its_plugin_a_repeated_id_and_an_illegal_wire_name` becomes `begin_run_drops_a_tool_outside_its_plugin_and_a_repeated_id`, and the `wire` entries leave the JSON tool helpers. In `crates/promptforge-internal/types/src/tools/tests.rs`, the wire-name assertions and invalid-wire-name tests go; the duplicate-id tests stay. Run `cargo nextest run --locked -p promptforge-types -p harness-runner --all-features`.
- Removal checks:
  - `git grep -nE "Package \{" -- crates AGENTS.md ':!vibe'` finds only `pub struct Package {`, `impl fmt::Debug for Package {`, and struct expressions inside `crates/promptforge-plugin/src/plugin.rs`.
  - `git grep -nE "fn (offered_)?binding\(" -- crates/promptforge-internal/lua/src/handles.rs` finds only the two inherent `pub fn` methods on `ToolSet`.
  - `git grep -n "wire_name" -- crates ':!vibe'` finds only `lua/src/tools/userdata.rs` and model-client or protocol names unrelated to `ToolDescriptor`; `crates/promptforge/public-api.txt` has no `ToolDescriptor::wire_name` and no `InvalidWireName`.
- Exit criteria, all passing at the end:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` and `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - With `CARGO_BUILD_WARNINGS=deny`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features`.
  - `cargo fmt --all --check`.
  - With `RUSTDOCFLAGS="-D warnings"`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
  - `cargo +nightly-2026-09-05 xtask api --check`.
  - `cargo test -p build-xtask`.
  - `cargo hakari verify` and `cargo deny check`.
  - In `crates/workshop`: `npm run build --workspace ui`, then `npm test --workspaces --if-present` (it runs `ui/test/docs-claims.mjs` over `AGENTS.md` and `## Invariants` text) and `npm run typecheck --workspaces --if-present`. The boot tests load the built bundle, so build first.

</verification-contract>
<decision-record>

## Decision Record

- User-resolved architecture choices:
  - D1-1: make `Package` `#[non_exhaustive]` with a `const fn` constructor and setters, now. Rejected: freezing the four fields and growing through defaulted `Plugin` methods (it can't carry data the Host reads before `construct`, such as package pins), and leaving it as is (the break only grows).
  - D1-22: remove `ToolDescriptor::wire_name`, a public API change in `promptforge` and `promptforge-plugin`, timed with D1-1 so Plugin crates change once.
  - C-1: delete `ToolView::binding` with D1-2.
  - D1-22 follow-on: remove the public `ToolIdErrorKind::Separator` variant together with `validate_identifier`, in the same `promptforge::tools` break. `validate_identifier` (`crates/promptforge-internal/types/src/tools/ids.rs`) is its only producer; `ToolId` parsing reports only `SegmentCount`, `Empty`, and `Control`. Rejected: keeping a public variant nothing returns. `ToolIdError::field` stays public and now always reports `id`; the crate-private `ToolIdError::reason`, whose only caller is the removed wire-name check, is deleted.
  - The operator states that breaking the public API costs essentially nothing: `promptforge`, `promptforge-plugin`, and `harness` have no consumers outside this repository. So when a removal this plan approves leaves a public item unreachable or unused, removing that item in the same change is settled and needs no further confirmation.
- Reversible decisions:
  - `Package` keeps its `pub` fields for reading. Rejected: getters, which add surface for no gain while the Host reads four fields.
  - `const fn` builders rather than a non-const builder, so labels stay compile-time constants. `const fn` may take function-pointer parameters on the workspace's stable toolchain.
  - Delete both dead `ToolView` methods rather than keep them for symmetry: neither has a production caller.
  - No structural ratchet: rustc E0063 and E0639, the existing tests, and `xtask api --check` already protect these contracts.
- Assumptions and risks:
  - D1-1 and D1-22 edit the same Plugin crates and fixtures; doing them in one sequence avoids conflicting edits.
  - Out-of-repository Plugin crates don't exist yet (`publish = false`), so the break reaches no one outside the repository.

### Deferred and Out of Scope

- A public API listing for `promptforge-plugin`. Revisit when the contract crate gains a consumer outside this repository.
- The Engine's `tracing` dependency for offering leave-outs (C-2). Revisit if the Engine must stay free of logging, or Hosts need left-out tools reported as run events.
- `Plugin` and `Invariants` wording that says `Drop` runs when the Host drops the context; it runs when the last run or call holding the Plugin also ends (D1-17). Revisit with the first Plugin whose `Drop` has an observable effect, such as an MCP server.
- The stale "`RunServices` hands a Plugin" comment in `crates/promptforge-internal/types/src/cancel.rs` (D1-23). Revisit when a change touches that file.
- A Plugin-relative path accessor on `ToolId` for nested ids (D1-16), and avoiding the per-call offering clone (D1-20). Revisit with large or nested MCP tool sets.
- A usable Plugin with zero valid tools being reported as missing (D1-7). Revisit when MCP servers can list zero tools.
- The Lua tool handle's script-visible `wire_name`. Revisit if scripts stop needing it.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (plain `cargo build` builds only the default member `crates/gateway/app`). The desktop app is explicit: `cargo build --locked -p workshop`. The headless shape gate `cargo check -p gateway --no-default-features` is the one standalone `cargo check` AGENTS.md allows. Prerequisites that CI installs first: `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` (UI bundles build into `OUT_DIR`); before any Workshop clippy or test, CI builds `cargo build --locked -p gateway --no-default-features` and stages it with `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`. Windows links with `rust-lld` and the static CRT (`.cargo/config.toml`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features --test <it|suite> <filter>` for an integration suite, or `cargo nextest run --locked -p <crate> --all-features --lib <filter>` for unit tests. Integration binaries are named `it`, except `suite` in the `promptforge` and `harness` facades. Keep `--all-features`: `promptforge-plugin`'s `tests/it` is `#![cfg(feature = "test-support")]` and compiles to nothing without it. For `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features`, as CI does.
- Component test command pattern: `cargo nextest run --locked -p <crate> [-p <crate> ...] --all-features` (Workshop trio without `--all-features`). Example family grouping for the Plugin contract, its Plugins, and the Harness: `cargo nextest run --locked -p promptforge-plugin -p plugin-web -p plugin-user-input -p harness-runner -p harness -p harness-gateway-client --all-features`. Structural checks alone: `cargo test -p build-xtask` (`cargo xtask tidy` prints the same report). Terminology check alone: `node --test test/docs-claims.mjs` run from `crates/workshop/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` (AGENTS.md and `.github/workflows/ci.yml`). The first run includes `build-xtask`'s structural checks. CI also runs `cargo nextest run --locked -p workshop-server --features headless`, `npm test --workspaces --if-present` in `crates/workshop` (includes `ui/test/docs-claims.mjs`), and `npm test` in `crates/gateway/config-ui/ui`. Nextest config is `.config/nextest.toml` (60s slow timeout, whisper suites in a `heavy` group). cargo-nextest 0.9.128 is installed locally.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` (in PowerShell, set `$env:CARGO_BUILD_WARNINGS = "deny"` first). Workspace lints deny clippy `all` and `pedantic`, `unwrap_used`, `expect_used`, `allow_attributes`, and `allow_attributes_without_reason`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn, which the deny setting makes fatal. When dependencies change: `cargo deny check` and `cargo hakari verify` (both installed locally); CI adds `cargo audit`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; the pre-commit hook runs the same).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus each facade with default features: `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps` and `RUSTDOCFLAGS="-D warnings" cargo doc -p harness --no-deps`. CI also runs, with the same flags, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items` and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Facade surface check: `cargo +nightly-2026-09-05 xtask api --check` (pin in `crates/build-xtask/src/api/toolchain.rs`), compared against `crates/promptforge/public-api.txt`; `--bless` rewrites the listing.
- Test placement and naming conventions:
  - Unit tests live in a sibling file named `<module>-tests.rs` (or `<module>-tests-<label>.rs`), wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`, for example `crates/promptforge-plugin/src/context-tests.rs`. Three or more test files for one module move into `src/<module>/tests/` or sit inside the module's subdirectory, as in `crates/plugin-web/src/fetch/tests-body.rs` and `crates/harness-gateway-client/src/transport/tests/`.
  - Integration tests form one binary per crate at `tests/it/main.rs`, which declares one `mod` per topic file (`tests/it/ask.rs`, `tests/it/effect_loop-broker.rs`, `tests/it/harness-stop-timing.rs`). The `promptforge` and `harness` facades use `tests/suite/main.rs` and test only through public items. Helpers go in `tests/it/support.rs` or `tests/common/`, data in `tests/fixtures/`.
  - Test helpers offered to other crates sit behind a `test-support` feature (for example `promptforge_plugin::testing::TestCall`, used from the Plugins' `[dev-dependencies]`); Workshop crates use `test-fixtures`. A leak guard rejects `test-support` in any non-dev table.
  - `clippy.toml` allows `unwrap`/`expect` inside tests; helper code outside `#[test]` uses `#![expect(clippy::expect_used, reason = "...")]`.
  - No doctests anywhere: compiled code blocks in doc comments are banned; use `text` fences.
- Directory map:
  - `crates/`: every Rust crate plus the npm UI packages. Manifestless containers hold each family's private crates and are listed explicitly in the root `Cargo.toml` members.
  - `crates/promptforge` (Engine facade) and `crates/promptforge-internal/` (`engine`, `lua`, `model-client`, `parser`, `types`, `vfs`).
  - `crates/promptforge-plugin` (the Plugin contract: `Package`, `Plugin`, `ToolContext`, `HostServices`, `ServiceKey`), `crates/plugin-web` (`web/fetch`, `web/search`), `crates/plugin-user-input` (operator input).
  - `crates/harness` (Harness facade, re-exports `HostContext`, `InstallError`, `HostServices`, `PluginId` under `harness::plugin`), `crates/harness-internal/runner` (`harness-runner`: per-run Harness, effect loop, `HostContext`, recorder), `crates/harness-gateway-client` (`GatewayBroker`, web search provider for the Gateway).
  - `crates/gateway/` (app = `gateway` crate building `promptforge-gateway`; `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the `stt/` subsystem: `api`, `engine`, `backend-whisper`, `whisper-ffi`), plus public `crates/gateway-api-types` and `crates/gateway-api-discovery`.
  - `crates/workshop/` (`desktop` = Tauri app crate `workshop`, `server`, `server-api`, `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, `workspace`; npm packages `ui`, `look`, `platform`).
  - `crates/shared-error-source`, `crates/shared-loopback`, `crates/shared-ui` (TypeScript and CSS, excluded from Cargo).
  - `crates/build-*` tooling: `build-ceiling` (500-line file check), `build-xtask` (`cargo xtask`: structural checks, `api`, `tidy`, `site`, `new-crate`), `build-workshop` (`cargo workshop`), `build-ui`, `build-user-guide`, `build-llama-cuda`; `crates/workspace-hack` (cargo-hakari).
  - `guide/` (user guide books `gateway`, `language`, `workshop`, plus `chrome` and `landing`), `prompts/` (sample prompts), `tools/` (Node scripts for sidecar staging and a TTS live test), `vibe/` (past plans), `images/` (README art).
  - `.github/workflows/` (CI), `.githooks/` (pre-commit fmt; pre-push headless check, clippy, deny), `.config/` (nextest, hakari), `.cursor/rules/` (Workshop architecture and SPA rules).
- Component boundaries (enforced by `crates/build-xtask/src/product.rs` through `cargo test -p build-xtask`):
  - Engine: `promptforge` depends on all six internal crates; `promptforge-engine` on `lua`, `model-client`, `parser`, `types`, `vfs`; `parser` on `lua`, `types`; `lua` on `model-client`, `types`, `vfs`; `model-client` on `types`; `types` and `vfs` on nothing. Sans-I/O: no `tokio`, `tokio-util`, `async-trait`, or `reqwest` in non-dev tables. No dependency on gateway, Workshop, Harness, or Plugin crates.
  - Plugin contract: `promptforge-plugin` depends only on `promptforge-types` and `promptforge-vfs` (its container exception) and is bound by the Engine manifest guard. `plugin-*` crates depend only on `promptforge-plugin`, `shared-*`, `workspace-hack`, and outside libraries.
  - Harness: `harness` depends on `harness-runner`, `promptforge`, `promptforge-plugin`; `harness-runner` on `promptforge` and `promptforge-plugin`; `harness-gateway-client` on `harness`, `promptforge`, and `plugin-web`, the one allowed edge into a Plugin (it implements web's `SearchProvider`). No `tokio` in normal dependencies; no gateway, shared, or Workshop crates.
  - Gateway: `gateway` sits over the `gateway/` container; outsiders see only `gateway-api-types` and `gateway-api-discovery`; inside the family only `gateway-stt` may name the `stt/` subsystem. No promptforge, Workshop, or Harness dependencies.
  - Workshop: `workshop-server` is the hub (depends on `harness`, `harness-gateway-client`, `plugin-web`, `plugin-user-input`, `promptforge`, `workshop-agents`, `workshop-run-log`, and the tier crates); `workshop-agents` depends on `harness`, `plugin-user-input`, `promptforge`; `workshop-run-log` on `harness`. The desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`. Gateway access only through its public pair.
  - Outside a family, only the public crates `promptforge`, `promptforge-plugin`, `harness`, and `harness-gateway-client` may be named. `shared-*` crates depend on no product crate.
- Conventions summary:
  - Edition 2024, resolver 3, stable toolchain. Every member sets `[lints] workspace = true` and depends on `workspace-hack`; versions live in root `[workspace.dependencies]` with a comment justifying each pin, and members use `workspace = true`.
  - Every crate's `build.rs` calls `build_ceiling::check()`, failing the build for any `.rs` file over 500 physical lines in `src`, `tests`, `benches`, or `examples`.
  - Each crate root doc carries a `## Invariants` section stating its dependency rules and contract; Plugin and Harness crates follow this.
  - Source directories stay flat: one or two child files sit beside the parent as `parent-label.rs` with `#[path]`; three or more become a subdirectory.
  - Errors are `thiserror` enums; error and status text is written for model consumption, naming required versus actual.
  - Suppressions use `#[expect(lint, reason = "...")]`; `#[allow]` is denied. `unsafe_code` is denied outside its owned boundary. No `doc(hidden)` under the facades or their containers. Process-global installers are disallowed outside binary entry points (`clippy.toml`).
  - Engine, Harness, Host, and Plugin are capitalized with one meaning each, never "capability", "pack", or "addon"; `crates/workshop/ui/test/docs-claims.mjs` enforces this in `AGENTS.md`, `## Invariants` docs, and `.cursor/rules`.
  - Comments explain only non-obvious constraints; workarounds cite an upstream issue URL. Behavior changes ship with tests in the same change.
  - JSON reaching a recorder round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys).
  - Commits use imperative, behavior-describing subjects (`Install Plugins once per Host and dispatch calls by name`); a finished plan closes with `Close plan: <slug>`. Text files use LF (`.gitattributes`).

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Delete the dead `ToolView` binding lookups [completed]

- Component: `none`

- Covers D1-2 and C-1 (todo `toolview-dead-methods`). Internal only; no public API or Plugin crate changes.
- `crates/promptforge-internal/lua/src/handles.rs`: delete `fn binding` and `fn offered_binding` from `trait ToolView` and from `impl ToolView for Mutex<ToolSet>`, plus any import only they used. Keep the inherent `ToolSet::binding` and `ToolSet::offered_binding`, and keep `ToolView::{bindings, always, offered}`, which `tool_set_snapshot` in `crates/promptforge-internal/engine/src/execute/context.rs` reads.
- `crates/promptforge-internal/lua/src/handles-tests.rs`: `the_view_snapshots_the_offering` keeps its `ToolView::offered` assertion and drops its two `ToolView::offered_binding` assertions. Leave `an_offered_binding_is_found_by_its_name_and_never_as_a_declared_slot` unchanged; it covers the inherent lookup every production caller uses (`scheduler/tool_call.rs`, `lua/src/tools.rs`, `lua/src/vm/state.rs`).
- Verify:
  - `cargo nextest run --locked -p promptforge-lua -p promptforge-engine --all-features`.
  - With `CARGO_BUILD_WARNINGS=deny`: `cargo clippy -p promptforge-lua -p promptforge-engine --all-targets --all-features`.
  - `git grep -nE "fn (offered_)?binding\(" -- crates/promptforge-internal/lua/src/handles.rs` finds only the two inherent `pub fn` methods on `ToolSet`.
- Commit: one commit with the code and test edit, subject `Delete the ToolView binding lookups nothing calls`.

</step-1>

<step-2>

### Step 2: Build Plugin labels with `Package::new` and drop `ToolDescriptor::wire_name`

- Component: `none`

- Covers D1-1 and D1-22 in one commit, so every Plugin crate and Harness fixture changes once (todos `package-const-ctor`, `remove-wire-name`, and `exit-gates`). Includes the `ToolIdErrorKind::Separator` removal recorded in the Decision Record.
- `Package` (D1-1), `crates/promptforge-plugin/src/plugin.rs`:
  - Add `#[non_exhaustive]`. Keep `#[derive(Clone, Copy)]`, the four `pub` fields, and `impl fmt::Debug for Package`.
  - Add `#[must_use]` `const fn`s: `Package::new(name, construct)` (prelude `None`, needs `&[]`), `Package::prelude(self, &'static str)`, and `Package::needs(self, &'static [ServiceId])`. Put the existing `#[expect(clippy::type_complexity, reason = ...)]` on `new` for its `construct` parameter; add no public type alias.
- Label migration, 20 struct-literal sites in 11 files, each becoming `Package::new(..)` with setters only where non-default:
  - Production: `crates/plugin-web/src/web.rs` (`PACKAGE`, no setters) and `crates/plugin-user-input/src/lib.rs` (`PACKAGE`, `.prelude(PRELUDE).needs(NEEDS)`).
  - Tests: `crates/promptforge-plugin/tests/it/package.rs`, `crates/harness/tests/suite/asker.rs`, `crates/harness-internal/runner/tests/it/{asker,prepare-host-services,prepare-input,scripted,tool_context}.rs`, the body of `echo_package` in `crates/harness-internal/runner/tests/it/prepare.rs`, and ten in `crates/harness-internal/runner/src/host-tests.rs`, including the body of the `package` helper.
- Docs for D1-1: the literal-form text in `crates/promptforge-plugin/src/{lib,plugin}.rs`, the crate docs of `plugin-web` and `plugin-user-input`, and any `AGENTS.md` or `## Invariants` text the `Package {` check below finds.
- New test `a_package_from_new_has_no_prelude_and_no_needs_until_set` in `crates/promptforge-plugin/tests/it/package.rs`: one `const` label from `Package::new(..)`, one with `.prelude(..).needs(..)`, asserting every field of each.
- `wire_name` removal (D1-22) in the types crate, `crates/promptforge-internal/types/src/tools/`:
  - `descriptor.rs`: drop the `wire_name` field and parameter, so `ToolDescriptor::new(id, description, parameters_schema)`; reword the type docs and the module docs in `types/src/tools.rs` that mention a wire name.
  - `registry.rs`: drop the wire-name check and the `validate_identifier` import from `ToolCatalog::new`, `ToolCatalogError::InvalidWireName`, `ToolCatalogErrorKind::InvalidWireName`, their match arms, and the doc text naming wire names. `ToolCatalog::new` then refuses only repeated ids.
  - `ids.rs`: delete `validate_identifier`, `ToolIdError::reason`, and `ToolIdErrorKind::Separator`; reword the `ToolIdErrorKind`, `ToolIdError`, `ToolIdError::field`, and module text so it describes ids only.
- `wire_name` removal outside the types crate:
  - `crates/harness-internal/runner/src/host-run.rs`: per-tool validation keeps containment (the tool sits under its Plugin's name) and the shared `seen` set; drop the `ToolCatalog::new(std::slice::from_ref(tool))` step and the doc text requiring a wire name a model transport accepts.
  - `crates/promptforge-internal/engine/src/test_support/tools.rs`: drop `TestTool::wire_name`, its use in `TestTool::descriptor`, and the wire-name text in the trait docs and in the `# Errors` section of `TestToolTable::catalog`.
  - Remove every `TestTool::wire_name` impl in `crates/promptforge-internal/engine/src/execute/tests.rs` and `execute/tests/{fixtures,model_task_notices,offering,tool_call_access}.rs`, `execute/tests/scheduler/failures-script-tools.rs`, and `execute/tests/suite/exec_flow/run_setup.rs`. `ScopedFixtureTool` in `fixtures.rs` loses its `wire_name` field and constructor parameter; update its 12 callers in `execute/tests/{debug_and_counts,full_id_calls,local_tools,observations,tool_loop,tool_scoping}.rs`.
- Update every `ToolDescriptor::new(` call (34 call lines in 29 files):
  - Production: `crates/plugin-web/src/web.rs` (two), `crates/plugin-user-input/src/ask.rs`, `crates/promptforge-internal/engine/src/execute/bindings.rs`.
  - Example: `crates/promptforge/examples/greeter.rs`.
  - Tests: `crates/harness-gateway-client/src/wire/request-tests.rs`; `crates/harness-internal/runner/src/host-tests.rs`; `crates/harness-internal/runner/tests/it/{asker,prepare-host-services,prepare-input,prepare,scripted,support,tool_context}.rs`; `crates/harness/tests/suite/asker.rs`; `crates/promptforge-internal/engine/src/{execute/run/tests-drops,lua/tests,test_support/tools,tools-tests}.rs`; `crates/promptforge-internal/lua/src/{dispatch-tests,handles-tests,tests,tools/tests}.rs`; `crates/promptforge-internal/types/src/tools/tests.rs`; `crates/promptforge-plugin/tests/it/package.rs`; `crates/promptforge/tests/suite/{effect,event,greeter,prepare,tools}.rs`.
- Tests that read, assert, or name the wire name:
  - Drop `wire_name` assertions in `crates/plugin-user-input/tests/it/ask.rs`, `crates/promptforge-internal/engine/src/tools-tests.rs`, and `crates/promptforge-plugin/tests/it/package.rs`. In `crates/plugin-web/src/web-tests.rs`, assert the tool ids instead of wire names and find `web_fetch` by id.
  - `crates/promptforge-internal/types/src/tools/tests.rs`: drop the `wire_name` parameter of `catalog_descriptor` and the `wire_name` assertions in `a_descriptor_has_the_tools_surface_and_round_trips_through_serde` and `catalog_preserves_order_and_first_match_lookup`; rename `catalog_lookup_uses_stable_identity_not_wire_name` so its name no longer contrasts with a wire name; delete `catalog_rejects_illegal_wire_name`. Keep `catalog_rejects_duplicate_tool_ids`, and keep the legacy JSON in `a_descriptor_logged_without_survives_stop_reads_as_not_surviving`, which still carries `"wire_name"` and now also proves old descriptors deserialize.
  - Delete `a_catalog_refuses_a_slashed_wire_name_the_descriptor_accepted` in `crates/promptforge/tests/suite/tools.rs`.
  - `crates/harness-internal/runner/src/host-tests.rs`: rename `begin_run_drops_a_tool_outside_its_plugin_a_repeated_id_and_an_illegal_wire_name` to `begin_run_drops_a_tool_outside_its_plugin_and_a_repeated_id`, remove its illegal-wire-name case, and drop the `"wire"` key from the JSON tool helpers and fixtures.
- Unchanged on purpose: the Lua tool handle's `wire_name` (`crates/promptforge-internal/lua/src/tools/userdata.rs` and the script check in `lua/src/tests/tool_scoping.rs`), model-client tool-schema validation (`model-client/src/detail.rs`, `client/wire.rs`, `client/tests.rs`), and the `wire name` wording in `lua/src/protocol/request.rs`, the scripted-chat test support, and the gateway cloud providers.
- Regenerate `crates/promptforge/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`. The diff must only drop `ToolDescriptor::wire_name`, both `InvalidWireName` entries and their fields, and `ToolIdErrorKind::Separator`, and change the `ToolDescriptor::new` signature.
- Verify:
  - Focused: `cargo nextest run --locked -p promptforge-plugin --all-features`, then `cargo nextest run --locked -p promptforge-types -p harness-runner --all-features`, then the family `cargo nextest run --locked -p promptforge-plugin -p plugin-web -p plugin-user-input -p harness-runner -p harness -p harness-gateway-client -p promptforge -p promptforge-engine -p promptforge-lua --all-features`.
  - `git grep -nE "Package \{" -- crates AGENTS.md ':!vibe'` shows no struct literal outside `crates/promptforge-plugin/src/plugin.rs`. Expected matches: `pub struct Package {`, `impl fmt::Debug for Package {`, and struct expressions in `plugin.rs`; `InvalidPackage {` in `crates/harness-internal/runner/src/host.rs` and `host-tests.rs`; and the `-> Package {` signatures of the `package` helper in `host-tests.rs` and `echo_package` in `runner/tests/it/prepare.rs`.
  - `git grep -n "wire_name" -- crates ':!vibe'` matches only `lua/src/tools/userdata.rs`, `lua/src/tests/tool_scoping.rs`, `model-client/src/client/tests.rs`, and the legacy JSON in `types/src/tools/tests.rs`. `crates/promptforge/public-api.txt` has no `wire_name`, `InvalidWireName`, or `ToolIdErrorKind::Separator`.
  - This step's verification, not its coding, runs every Exit criteria command in the Testing Plan with both steps in the tree. It follows the Project Survey prerequisites: `npm ci` for the two UI packages, the headless gateway build and sidecar staging before Workshop clippy or tests, and `npm run build --workspace ui` before `npm test`. In PowerShell, set `$env:CARGO_BUILD_WARNINGS` and `$env:RUSTDOCFLAGS` before the commands that need them.
- Commit: one commit with all code, test, doc, and `public-api.txt` edits, subject `Build Plugin labels with Package::new and drop unused tool wire names`.

</step-2>

</execution-plan>
