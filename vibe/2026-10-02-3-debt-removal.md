---
name: Debt removal changes 2-5
overview: "Remove the debt that changes 2 to 5 (upstream/master b845508c to 1f0554a2) added and that changes 6 to 9 will not delete: align the public CompletionError message contract with the shipped classifier and catalog, fix the guide's search row order, and correct stale layout prose. Debt that changes 6 and 8 dissolve is deferred to them."
todos:
  - id: failure-contract
    content: "D1-3, D1-12: catalog regression test first; Gateway text to detail in catalog.rs; align CompletionError and classify_http_failure docs with the shipped messages"
    status: pending
  - id: guide-order
    content: "D1-4: fix the search row order sentence in guide chapter 13 and regenerate the export"
    status: pending
  - id: layout-prose
    content: "D1-6: fix archdoc lines 10 and 23, replace the root AGENTS.md import pointer in the new-crate template and surviving Invariants blocks, name the platform boundary guard"
    status: pending
  - id: exit-checks
    content: Run the exit checks once after all three items
    status: pending
isProject: false
---

# Debt removal: changes 2 to 5

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `promptforge`. Baseline `b845508c` (`upstream/master`, also the merge base), endpoint `1f0554a2`, 25 target commits. Every line reference in this plan is against `1f0554a2`; if `HEAD` has moved, locate each site by its quoted text.
  - The target work covers four changes:
    - change 2, the Host-owned run recorder (`6a901b56` to `2f7c6d4c`);
    - change 3, the failure vocabulary and precheck (`3829235d` to `4a7b9f9e`);
    - change 4, `harness-gateway-client` (`4538b68d` to `6b9192d7`, plan `vibe/2026-10-02-1-gateway-client.md`);
    - change 5, host services and `harness-web` (`5f8b9bea` to `1f0554a2`, plan `vibe/2026-10-02-2-host-services.md`).
  - Changes 6 to 9 of the host-boundary effort run next: the inference broker, streaming in the broker, removing sessions, and dropping tokio. Debt that those changes delete or rewrite is deferred to them, not fixed here.
- Cleanup goals:
  - The public `CompletionError` message contract describes what the standard classifier produces, and no Gateway-supplied text reaches a catalog failure's message.
  - The guide states the search row order the tool produces.
  - The architecture record and the `## Invariants` import pointers name crates and rules that exist.
- Non-goals:
  - No change to classifier messages, error kinds, or the guide's failure table.
  - No new structural checks.
  - No work on code that change 6 or change 8 deletes: sessions, discovery, the built-in `chat` agent, the session transcript, and the search mirrors.
- Success criteria:
  - The `CompletionErrorKind::phrase`, `CompletionError`, and `CompletionError::new` docs, and the `classify_http_failure` doc, allow the ` (status N)` suffix and the 401 and 403 credentials message, matching `crates/promptforge/src/model.md:363`.
  - No catalog failure message contains Gateway-supplied text, such as a model name or id; the catalog tests prove it for every failure site.
  - Guide chapter 13 and its export state `title`, `url`, `description` as the row order.
  - Outside `vibe/`, `harness-webfetch` appears only as the unchanged User-Agent string, and `vibe/archdoc.md` no longer names it. The archdoc names all three public Harness crates. Neither the new-crate template nor any `## Invariants` block outside `harness-sessions` tells a reader to read the root `AGENTS.md` before adding an import.
  - The exit checks in the Testing Plan pass.

## Functional Specification

### Debt Inventory

- Debt added, fixed here:
  - **D1-3 (introduced, `3829235d`, `1efd9441`, `4538b68d`, `14990ac9`).** The public rule says a message is the kind's fixed phrase, and only `MalformedResponse`, `EmptyReply`, and `Unavailable` may extend it with `: `. The rule appears in three places in `crates/promptforge-internal/model-client/src/model/error.rs`: `phrase` docs at `:57-63`, `CompletionError` at `:132-135`, and `new` at `:172-181`. But `classify_http_failure` (`crates/harness-gateway-client/src/wire/classify.rs`) appends ` (status N)` to every kind (`http_error`, `:159-162`). For 401 and 403 it returns `Unavailable` with "the model backend did not accept the credentials" (`:104-105`). Its own doc at `:56-57` is also wrong for 401 and 403. Impact: third-party brokers, which change 6 makes the norm, follow docs the standard broker contradicts. Reversal cost: none for docs. Target: the docs describe the shipped messages.
  - **D1-12 (introduced, `1efd9441`), same cause as D1-3.** `failure::malformed` (`crates/harness-gateway-client/src/failure.rs:41-43`) says "Provider text never goes in `specific`". The `CompletionError` docs say provider text never goes in the message. But `fetch_model_catalog` (`crates/harness-gateway-client/src/catalog.rs:200-226`) puts Gateway-supplied text into messages at three call sites:
    - the model name in the zero-token message (`:203-208`);
    - the model name in the no-thinking-mode message (`:209-214`);
    - the `ModelCatalogError` display, whose `DuplicateId` variant echoes `{server}/{name}` (`:222-226`; `crates/promptforge-internal/types/src/models.rs:128`).

    The fourth `malformed` call, for an invalid id (`:200-202`), is clean. `ModelIdError`'s display is "invalid model id: {field} {reason}", built only from static text (`models.rs:113-120`), so it carries no Gateway text. `ModelId` refuses control characters but has no length bound, and the catalog body cap is 16 MiB. Target: fixed text in the message, and the Gateway-supplied text in `detail`.
  - **D1-4 (introduced, `d25a5518`).** `guide/src/language/13-web-fetch-and-search.md:473` says rows carry "a non-empty `url`, a `title`, and a `description`, in that order". The example at `:470` and the tool's output put `title` first: `harness_web::SearchResult` field order, pinned by `crates/harness-web/src/search-tests.rs:231`. Target: the sentence matches the output.
  - **D1-6 (worsened, corrective passes `6b9192d7` and `5f8b9bea`, still stale at the endpoint).** Hand-written prose names crates and rule locations and goes stale on each move:
    - `vibe/archdoc.md:23` (A3) names `harness-webfetch`, which `bca2e22d` deleted.
    - `vibe/archdoc.md:10` says the Harness's public surface is `harness`, while `PUBLIC_HARNESS` (`crates/build-xtask/src/product.rs`) lists `harness`, `harness-gateway-client`, and `harness-web`.
    - The `## Invariants` blocks of 22 crates (21, plus `harness-sessions`, which change 8 deletes) and the template at `crates/build-xtask/src/new_crate.rs:73` say to read the root `AGENTS.md` before adding an import. `5f8b9bea` cut the root import rules, and the template plants the pointer in every new crate.
    - `crates/workshop/platform/AGENTS.md:3` refers to "the guard", whose name the trim cut. It is `test/boundary.mjs`.

    Target: each statement names what exists, and the import pointer names the enforcing check.
- Debt added, deferred (see Deferred and Out of Scope):
  - D1-1: the built-in `chat` agent requires `promptforge/web`, which only a crate above the Harness supplies. Deferred to change 8.
  - D1-2: the search request and result types are mirrored across `harness-gateway-client`, `harness-web`, and Workshop. Deferred to change 6.
  - D1-5: a recorder failure after `begin_run` drops the run from the session's `run_ids` and transcript. Deferred to change 8.
- Cheap fixes: none separate. The platform "the guard" line has D1-6's cause and is fixed with it.
- Exposed pre-existing debt: none.
- Rejected candidates (20):
  - 9 residual but acceptable: deliberate design or interim states with a scheduled owner (D1-11, D1-15 to D1-22).
  - 6 weak or speculative: no demonstrated failure (D1-7 to D1-10, D1-13, D1-14).
  - 1 unrelated pre-existing: `TursoRecorder`'s single lock (D1-23).
  - 4 false: claims the endpoint disproves (D1-24 to D1-27).

</product-contract>
<implementation-contract>

## Technical Design

- Failure message contract (D1-3, D1-12):
  - `crates/promptforge-internal/model-client/src/model/error.rs`: amend the `CompletionErrorKind::phrase`, `CompletionError`, and `CompletionError::new` docs to state:
    - a failure built from an HTTP status appends ` (status N)` to the message;
    - a 401 or 403 is `Unavailable` with the message "the model backend did not accept the credentials";
    - provider text never enters the message and goes in `detail`.

    Match the wording of the facade page `crates/promptforge/src/model.md:363`, whose kind table already lists the credentials message (`:378`), so `model.md` needs no edit. No signature changes.
  - `crates/harness-gateway-client/src/wire/classify.rs:56-57`: make the `classify_http_failure` doc name the credentials message for 401 and 403.
  - `crates/harness-gateway-client/src/catalog.rs:203-226`: the three leaking `malformed` calls keep a fixed specific this crate wrote, such as "a model declares a zero-token context window". The model name (`id.name()`) and the `ModelCatalogError` display move into `CompletionError::with_detail` (`error.rs:232`; read back through `detail()`, `:304`). The invalid-id call (`:200-202`) stays as it is.
- Guide (D1-4): fix the sentence at `guide/src/language/13-web-fetch-and-search.md:473` to "a `title`, a non-empty `url`, and a `description`, in that order", and regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`.
- Layout prose (D1-6):
  - `vibe/archdoc.md:10`: the public surface is `harness`, `harness-gateway-client`, and `harness-web`.
  - `vibe/archdoc.md:23`: A3 names the fetch tool in `harness-web`.
  - Replace the import pointer with "`cargo test -p build-xtask` enforces the product and container boundaries.":
    - In the template `crates/build-xtask/src/new_crate.rs:73`, and in any test that pins the template text.
    - In the `src/lib.rs` `## Invariants` block of the 21 crates step 2 lists. Most sites wrap the sentence across `//!` lines, so find them with the wrap-tolerant pattern in the Testing Plan.
    - Skip `crates/harness-internal/sessions/src/lib.rs`, which change 8 deletes.
    - In `crates/workshop/server/src/lib.rs:34-36`, keep the pointer to `crates/workshop/server/AGENTS.md`, which still holds rules, and replace only the root pointer.
  - `crates/workshop/platform/AGENTS.md:3`: "the guard" becomes "the boundary guard (`test/boundary.mjs`)".
  - Replacement text in `## Invariants` blocks keeps the capitalized Engine, Harness, and Host terms that `crates/workshop/ui/test/docs-claims.mjs` checks.

</implementation-contract>
<verification-contract>

## Testing Plan

- D1-12, regression first, one test per catalog failure site in `crates/harness-gateway-client/src/catalog.rs`, written in the sibling `catalog-tests.rs` that step 1 moves the test module into (the 500-line file cap):
  - Extend `fetch_model_catalog_still_rejects_a_zero_context_window` (`:369`, model `broken`), and add cases for the missing thinking mode and the duplicate id. Each asserts that the message omits the Gateway-supplied name and that `detail()` holds it. These must fail before the fix.
  - Add a case for an invalid id (one holding a control character), asserting that the message omits the Gateway-supplied id. It passes before any change, because `ModelIdError` carries only static text. Keep it as a guard and change nothing at that site.
  - A leaking-site test that already passes before the fix proves that site's finding false: keep the test and skip that site's change.
  - Then run `cargo nextest run --locked -p harness-gateway-client --all-features`.
- D1-3: documentation only, and no new test. `classify-tests.rs` (`:220-226`, `:332`), `error-tests.rs:95`, and the guide table at `guide/src/language/16-limits-and-errors.md:506-512` already pin the shipped messages. Run:
  - `cargo test --locked -p promptforge-model-client -p promptforge -p harness-gateway-client --all-features --doc`
  - `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
- D1-4: `cargo nextest run --locked -p harness-web --all-features` (the rendering test pins the order) and `cargo xtask site --books-only`.
- D1-6:
  - `node --test crates/workshop/ui/test/docs-claims.mjs` and `cargo test -p build-xtask`.
  - From the repository root, `rg -l harness-webfetch --glob '!vibe/**'` lists only the User-Agent sites: `crates/harness-web/src/config.rs`, `crates/harness-web/src/config-tests.rs`, `guide/src/language/13-web-fetch-and-search.md`, and `guide/promptforge-language-guide.md`. `rg harness-webfetch vibe/archdoc.md` finds nothing.
  - The wrap-tolerant search ``rg -l -U 'Read(\s|//!)+(the(\s|//!)+)?(repository-root(\s|//!)+)?`AGENTS\.md`' crates`` lists only `crates/harness-internal/sessions/src/lib.rs`. Before the change it lists 23 files: the template, `harness-sessions`, and the 21 crates.
- Exit checks, once at the end:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - D1-3: loosen the documented rule to match the shipped classifier and the facade page, rather than change the messages. The docs grow a second message shape (status suffix, credentials message). Rejected: rewriting classifier messages, because the guide table and tests pin them and `4538b68d` made the phrases public API.
  - D1-12: move Gateway-supplied text into `detail`. Operators read the model name from `detail` instead of the message. Rejected: documenting validated model ids as allowed, because `ModelId` has no length bound.
  - D1-6: fix the prose and the template instead of adding a check. Root `AGENTS.md` requires approval for new structural checks, and a layout ratchet protects no product contract. `harness-sessions`' pointer is left for change 8 to delete with the crate.
  - The platform "the guard" line is folded into D1-6 as the same cause: hand-written prose orphaned by a trim.
- User-resolved architecture choices:
  - D1-1 is deferred to change 8, which moves the built-in `chat` agent, the conversation, and reattach into a chat layer above the Harness. Rejected: a Host-supplied built-in agent map on `HarnessConfig`, and having `discover()` hide agents the Host cannot run. Both build on discovery code change 8 deletes.
  - D1-2 is deferred to change 6, after which `harness-sessions` no longer depends on `harness-gateway-client`. Rejected: marking the public mirrors `#[non_exhaustive]` now, which removes the compile error Workshop's exhaustive struct literals give when a field is added.
  - Removing the unused `Clone` on `CapabilityRegistry` is left to change 8, which decides how a per-run Harness receives the registry.
- Assumptions and risks:
  - Changes 6 to 9 follow the report "Move the harness's I/O to the host in nine changes" (updated 2026-10-02), which also lists the debt carried into changes 6 and 8. If change 8 slips, D1-1 and D1-5 stay live longer.
  - Each later change moves crates again, so `vibe/archdoc.md` needs its docs step every time.

### Deferred and Out of Scope

- D1-1, the built-in `chat` agent needs web from above the Harness. Revisit at change 8: the chat layer registers web, and the facade tours are rewritten around it.
- D1-2, the mirrored search types. Revisit at change 6: `harness-gateway-client` implements `harness_web::SearchProvider`, and its search wire types go private.
- D1-5, the recorder failure after `begin_run`. Revisit at change 8: a run that fails after `begin_run` returns its run id and recorded events to the caller.
- `CapabilityRegistry`'s unused `Clone`. Revisit at change 8, with how a per-run Harness receives the registry.
- D1-20, `RunServices::insert_input_broker` names an unexported trait. Revisit at change 8, when the input broker moves to the Host.
- D1-22, the `GatewayClient` name collision and the docs-site entries for `harness-gateway-client` and `harness-web`. Revisit at change 6.
- D1-16 and D1-21, the copied SSE test helpers and the sessions dependency on the public client. Change 6 removes both.
- D1-7, the unbounded session transcript. Deleted with sessions at change 8.
- D1-13, untagged spawns in `harness-web`. Revisit at change 9.
- D1-23, `TursoRecorder`'s single lock. Pre-existing and out of scope.
- The manual Workshop run of a prompt declaring `promptforge/web` that searches and fetches, owed by change 5's exit criteria. Not debt; still to do.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <crate>` for each crate a step touches. Plain `cargo build` builds only `gateway`, the workspace's sole default member. The desktop app builds with `cargo build --locked -p workshop`, or with the Gateway sidecar staged through `cargo workshop` (alias for `build-workshop`). Clippy is the compile check; never run a standalone `cargo check --workspace` beside it. The one extra compile gate is `cargo check -p gateway --no-default-features`. Local toolchain: stable cargo 1.99, cargo-nextest 0.9.128, node 24, and the pinned `nightly-2026-09-05` are installed; the shell is PowerShell on Windows.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`, adding `--test it` to target a crate's integration binary (`--test suite` for `promptforge` and `harness`). The Workshop trio `workshop`, `workshop-server`, and `workshop-server-api` runs without `--all-features`. A focused doctest is `cargo test --locked -p <crate> --all-features --doc <name>`. A focused UI test is `node --test <file>` from the package directory (`crates/workshop/ui`, `crates/workshop/look`, `crates/workshop/platform`, or `crates/gateway/config-ui/ui`).
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, then `cargo test --locked -p <crate> --all-features --doc`, because nextest skips doctests. For the Workshop trio: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. CI also runs `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless`. Workshop UI packages: from `crates/workshop`, `npm run build --workspace ui` then `npm test --workspace <ui|look|platform>`. Gateway config UI: from `crates/gateway/config-ui/ui`, `npm run build` then `npm test`. Structural checks: `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. The workspace run includes `build-xtask`'s structural checks. UI: from `crates/workshop`, `npm ci`, `npm run build --workspace ui`, `npm test --workspaces --if-present`; from `crates/gateway/config-ui/ui`, `npm ci`, `npm run build`, `npm test`. CI fails if any build step dirties the git tree.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, and for the Workshop trio `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. UI typecheck: `npm run typecheck --workspaces --if-present` from `crates/workshop` and `npm run typecheck` from `crates/gateway/config-ui/ui`. Supply chain, in CI and the pre-push hook when installed: `cargo deny check`.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook). No TypeScript, JavaScript, or CSS formatter is configured.
- Docs command: set `RUSTDOCFLAGS` to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), then `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, the facades with default features `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. User guide: `cargo xtask site --books-only`; the guide's single-file exports regenerate with `cargo run --locked -q -p build-user-guide`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` against the committed `crates/promptforge/public-api.txt` (the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`), plus its nightly-only fixtures `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Test placement and naming conventions: Unit tests sit in a `#[cfg(test)]` module in a sibling `<stem>-tests.rs` wired with `#[path = "<stem>-tests.rs"]`, or in a `src/<module>/tests/` directory once there are three or more files (for example `promptforge-internal/engine/src/execute/tests/`). Integration tests compile as one binary per crate rooted at `tests/it/main.rs` (`tests/suite/main.rs` in `promptforge` and `harness`), which declares one `mod` per area; split areas use kebab siblings such as `effect_loop-recorder.rs`. A few crates keep standalone files instead, such as `gateway/stt/engine/tests/*.rs` and `build-workshop/tests/interruption.rs`. Helpers live in `tests/it/support.rs` or `tests/common/`, data in `tests/fixtures/`, and prompt programs in `tests/prompts/`. Test names are snake_case sentences stating the behavior, such as `records_are_events_then_effects_then_answers_per_step`; async tests use `#[tokio::test]`; tests often return `Result` rather than unwrap; test roots opt out of the unwrap and expect lints with `#![expect(..., reason = "...")]`. Test-only APIs sit behind a `test-support` feature (Engine and Harness crates) or `test-fixtures` (gateway and Workshop crates). UI tests use Node's built-in runner over `test/**/*.mjs` and `src/**/*.test.mjs`. New structural checks belong only in `build-xtask` and need explicit user approval.
- Directory map: `crates/` holds every Rust crate plus the TypeScript UI packages, grouped into families with manifestless containers: the Engine facade `promptforge` and its private crates in `promptforge-internal/` (`types`, `engine`, `lua`, `parser`, `vfs`, `model-client`); the Harness facade `harness`, its private crates in `harness-internal/` (`runner`, `capabilities`, `sessions`), and the public `harness-gateway-client` and `harness-web`; the Gateway in `gateway/` (`app`, `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` with `api`, `engine`, `backend-whisper`, `whisper-ffi`) plus the public `gateway-api-types` and `gateway-api-discovery`; Workshop in `workshop/` (Rust: `desktop`, `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, `run-log`; TypeScript npm workspace: `ui`, `look`, `platform`); shared crates `shared-error-source` and `shared-loopback`, and the TypeScript and CSS package `shared-ui` that the Gateway config UI consumes; tooling crates `build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`; and the cargo-hakari `workspace-hack`. `guide/` holds the user guide chapters under `guide/src/<set>/`, the generated `promptforge-*-guide.md` exports, and the mdBook books, chrome, and landing page. `prompts/` holds sample prompt programs. `tools/` holds the `cicerone.md` agent tool for facade rustdoc and Node scripts with `.test.mjs` siblings. `vibe/` holds `archdoc.md` and dated plans. `.github/workflows/` holds CI (`ci.yml`) and the release, nightly, and site workflows. `.githooks/` holds the pre-commit format check and the pre-push headless check, clippy, and deny. `.cargo/config.toml` defines the `cargo xtask` and `cargo workshop` aliases and the Windows `rust-lld` linker with static CRT. `.config/` holds `nextest.toml` and `hakari.toml`. `local/` is gitignored operator config, `images/` is README art, and `target/` and `target-msrv/` are build output.
- Component boundaries: Engine: `promptforge-types` and `promptforge-vfs` are leaves; `promptforge-model-client` depends on types; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; `promptforge-engine` on all five; the `promptforge` facade re-exports them. No Engine crate depends on Harness, Gateway, Workshop, or shared crates. Harness: `harness-capabilities` and `harness-gateway-client` depend only on `promptforge`; `harness-runner` on capabilities; `harness-sessions` on capabilities, gateway-client, and runner; the `harness` facade on capabilities, runner, and sessions; `harness-web` on `harness`. Harness crates reach the Engine only through `promptforge` and depend on no gateway or shared crate. Gateway: `gateway-api-types` is the leaf wire vocabulary; config, progress, and protocol build on it; routing on config and protocol; local on config, progress, protocol, routing, and `shared-error-source`; web-search on config and protocol; the STT chain runs `whisper-ffi` (the only unsafe boundary) and `stt-engine` up through `backend-whisper` to `gateway-stt`; the `gateway` app crate sits on top. Workshop: tier 0 vocabulary is `workshop-protocol`, `workshop-registry`, and `workshop-support`; `workshop-gateway`, `workshop-menu`, `workshop-status`, `workshop-user-state`, and `workshop-workspace` build on it; `workshop-run-log` depends on `harness`; `workshop-server` composes all of them with `harness`, `harness-gateway-client`, `harness-web`, `promptforge`, `gateway-api-discovery`, and `shared-loopback`; `workshop-server-api` wraps the server and the `workshop` desktop app depends on server-api. Workshop reaches the Gateway only through `gateway-api-types`, `gateway-api-discovery`, and its protocol, never Gateway internals. Tooling crates depend on no workspace crates. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, and the Harness bans.
- Conventions summary: Rust edition 2024 on stable with resolver 3 and rustfmt `style_edition = "2024"`. Every member inherits `[workspace.lints]`: clippy `all` and `pedantic` denied, `unwrap_used` and `expect_used` denied outside tests, `unsafe_code` forbidden, `missing_docs` and `unreachable_pub` warned, and broken intra-doc links denied. Lint suppressions use `#[expect(..., reason = "...")]` rather than `#[allow]`. Library errors are typed with `thiserror`; `anyhow` appears only in tooling, binaries, and Gateway app crates. Crate root docs that opt into tidy checks open with `//! ## Invariants`; those crates, and every `workshop-*` and `harness-*` crate, keep each file under 500 lines. Source directories are flat; one or two related files become `foo-bar.rs` siblings with `#[path]`, three or more become a `foo/` subdirectory. Dependency versions live in `[workspace.dependencies]` with comments justifying each pin, and every member depends on `workspace-hack`. Comments explain constraints only, and workaround comments cite an upstream issue URL. Engine, Harness, and Host are capitalized defined terms, checked by `crates/workshop/ui/test/docs-claims.mjs`. JSON reaching a recorder round-trips exactly through serde_json `float_roundtrip`, never `preserve_order`. Error messages are written for model consumption, naming required versus actual. Workshop CSS uses `--ws-*` tokens and the SPA never touches `localStorage`. Text files are LF except `.ps1` and `.bat`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Failure message contract [completed]

- Component: `none`

- Covers: D1-3 and D1-12 (todo `failure-contract`). Depends on nothing.
- Move the tests out first. `catalog.rs` is 431 lines and the `build-xtask` tidy check caps every `harness-*` file at 500, so the new cases cannot go inline. Move the inline `#[cfg(test)] mod tests` (`catalog.rs:229` to the end) unchanged into a new sibling `crates/harness-gateway-client/src/catalog-tests.rs`, wired from `catalog.rs` with `#[cfg(test)]` and `#[path = "catalog-tests.rs"] mod tests;`, the same shape as the crate's `search-tests.rs` and `wire/*-tests.rs`. After the move, locate tests by name rather than by line.
- Regression first, in `crates/harness-gateway-client/src/catalog-tests.rs`:
  - Extend `fetch_model_catalog_still_rejects_a_zero_context_window` (its `zero-token` assertion) to assert that `err.to_string()` does not contain the model name `broken` and that `err.detail()` does.
  - Add sibling `#[tokio::test]` cases served through `spawn_models`, one per remaining Gateway-text site in `fetch_model_catalog`. Name each with the `fetch_model_catalog_` prefix so the focused filter below selects it:
    - an entry with a `context` but no `thinking` (the no-thinking-mode message carries no model name);
    - two entries sharing one `id` (the inconsistent-catalog message carries no `ModelCatalogError::DuplicateId` `{server}/{name}`);
    - an entry whose `id` holds a control character (the invalid-id message carries no part of the Gateway-supplied id).

    The first three cases are the leaking sites. Each asserts `CompletionErrorKind::MalformedResponse`, a message free of the Gateway-supplied name, and `detail()` holding it. The invalid-id case asserts only `MalformedResponse` and a message free of the id. Its message, "invalid model id: name must not contain a control character", is the crate's own static text (`crates/promptforge-internal/types/src/models.rs:113-120`), so it passes before the fix and stays as a guard. Together the four cases prove that no catalog failure message contains Gateway-supplied text.
  - Run `cargo nextest run --locked -p harness-gateway-client --all-features fetch_model_catalog` before the fix and confirm the three leaking-site cases fail. If one of them passes before the fix, that site's finding is false: keep its test and leave that site unchanged.
- Fix `fetch_model_catalog` (`catalog.rs:203-226`): the three leaking `malformed` calls keep a fixed specific this crate wrote, such as "a model declares a zero-token context window". The model name (`id.name()`) and the `ModelCatalogError` display move into `CompletionError::with_detail` (`crates/promptforge-internal/model-client/src/model/error.rs:232`; `detail()` at `:304`). Leave the invalid-id call (`:200-202`) unchanged. Leave the byte-limit specifics in `read_catalog_body_capped` (`:57`, `:64`) unchanged too; this crate wrote them, which the contract allows.
- Docs in `crates/promptforge-internal/model-client/src/model/error.rs`: amend the docs on `CompletionErrorKind::phrase`, `CompletionError`, and `CompletionError::new` to state that a failure built from an HTTP status appends ` (status N)`, that a 401 or 403 is `Unavailable` with "the model backend did not accept the credentials", and that provider text never enters the message and goes in `detail`. Match the wording of `crates/promptforge/src/model.md:363`. Its kind table already lists the credentials message (`:378`), so `model.md` needs no edit. No signature changes.
- Docs in `crates/harness-gateway-client/src/wire/classify.rs`: make the `classify_http_failure` doc (`:56-57`) name the `CREDENTIALS_PHRASE` message for 401 and 403.
- Verify:
  - `cargo nextest run --locked -p harness-gateway-client --all-features`
  - `cargo test --locked -p promptforge-model-client -p promptforge -p harness-gateway-client --all-features --doc`
  - `cargo doc -p promptforge --no-deps` with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`)
  - `cargo +nightly-2026-09-05 xtask api --check`
- Commit: one commit holding the new `catalog-tests.rs`, the `catalog.rs` test-module move and fix, and the `error.rs` and `classify.rs` doc amendments.

</step-1>

<step-2>

### Step 2: Guide row order and layout prose [completed]

- Component: `none`

- Covers: D1-4 (todo `guide-order`), D1-6 (todo `layout-prose`), and the exit checks (todo `exit-checks`). Depends on nothing in step 1; it runs second so the exit checks cover both commits.
- Guide: change the sentence at `guide/src/language/13-web-fetch-and-search.md:473` to "a `title`, a non-empty `url`, and a `description`, in that order". Regenerate `guide/promptforge-language-guide.md` (the same sentence sits at `:8949`) with `cargo run --locked -q -p build-user-guide`; do not hand-edit the export.
- Archdoc: `vibe/archdoc.md:10` names the Harness's public surface as `harness`, `harness-gateway-client`, and `harness-web`, matching `PUBLIC_HARNESS` in `crates/build-xtask/src/product.rs`. `vibe/archdoc.md:23` (A3) names the fetch tool in `harness-web` instead of `harness-webfetch`.
- Import pointer: replace the root `AGENTS.md` pointer with "`cargo test -p build-xtask` enforces the product and container boundaries." in:
  - the template string at `crates/build-xtask/src/new_crate.rs:73`. No `build-xtask` test pins that text today; if one does by the time this runs, update it in this commit.
  - the `src/lib.rs` `## Invariants` block of these 21 crates:
    - Engine: `promptforge-internal/types`, `vfs`, `model-client`, `lua`, `parser`, `engine`
    - Harness: `harness-internal/capabilities`, `harness-internal/runner`, `harness-gateway-client`, `harness-web`
    - Workshop: `workshop/protocol`, `registry`, `support`, `gateway`, `menu`, `status`, `user-state`, `workspace`, `run-log`, `server-api`, `server`
  - Most of these sites wrap the sentence across `//!` lines; rewrap each edited bullet to the block's existing width.
  - In `crates/workshop/server/src/lib.rs:34-36`, keep the pointer to `crates/workshop/server/AGENTS.md` and replace only the root pointer.
  - Skip `crates/harness-internal/sessions/src/lib.rs`, which change 8 deletes.
  - Keep the capitalized Engine, Harness, and Host terms that `crates/workshop/ui/test/docs-claims.mjs` checks.
- Platform: in `crates/workshop/platform/AGENTS.md:3`, "the guard" becomes "the boundary guard (`test/boundary.mjs`)".
- Verify:
  - `cargo nextest run --locked -p harness-web --all-features` (the rendering test pins the row order) and `cargo xtask site --books-only`
  - `node --test crates/workshop/ui/test/docs-claims.mjs` and `cargo test -p build-xtask`
  - From the repository root, `rg harness-webfetch --glob '!vibe/**'` finds only the User-Agent string (`crates/harness-web/src/config.rs`, `config-tests.rs`, guide chapter 13, and its export), and `rg harness-webfetch vibe/archdoc.md` finds nothing.
  - ``rg -l -U 'Read(\s|//!)+(the(\s|//!)+)?(repository-root(\s|//!)+)?`AGENTS\.md`' crates`` lists only `crates/harness-internal/sessions/src/lib.rs`. This is the same wrap-tolerant search as in the Testing Plan.
- Exit checks, once, after this step's commit:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS` set to `-D warnings`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
- Commit: one commit holding the guide source and regenerated export, `vibe/archdoc.md`, the template, the 21 `lib.rs` edits, and `crates/workshop/platform/AGENTS.md`.

</step-2>

</execution-plan>
