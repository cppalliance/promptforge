---
name: Debt removal api firewall
overview: "Remove the four debts the promptforge API firewall commits (75245481..9eac5f3b) introduced or worsened: stale Lua chunk names, a surface listing blind to compatibility-breaking edits, the facade's test-only feature and module, and the doctest dev-dependency cycle that six crate notes deny."
todos:
  - id: step-1
    content: "Step 1 (chunk-names): repoint TASKS/FANOUT/MESSAGES chunk names to crates/promptforge-internal/lua/src/; content-match tests in coro-tests.rs and messages-tests.rs"
    status: pending
  - id: step-2
    content: "Step 2 (test-support): harness-capabilities suite on a store-only Run::step/resume loop; engine drive_tokio example on engine paths; delete facade test-support feature, module, and page; single-build xtask api; remove facade_shape cfg allowance; sweep ci.yml and AGENTS.md"
    status: pending
  - id: step-3
    content: "Step 3 (doctest-cycle): doctest-only facade dev-dependency exception in AGENTS.md line 35; amend the six crate statements"
    status: pending
  - id: step-4
    content: "Step 4 (listing-fidelity): non_exhaustive, kind, and private-field markers in api/render.rs; pinned-nightly fixtures; re-bless public-api.txt; exit gates"
    status: pending
isProject: false
---

# Debt removal: promptforge API firewall

<product-contract>

## Product Requirements

The API firewall commits (`75245481..9eac5f3b`) left four debts that are still in the tree. They are stale Lua chunk names, a surface listing that misses compatibility-breaking edits, a test-only feature and module on the public facade, and doctest dev-dependencies that contradict the repository's dependency rule and six crate notes. The people affected are contributors and agents reading the code, reviewers of the facade surface, and the harness test suites; no outside host exists yet. When the plan is done, the facade has no test surface, the listing records what hosts rely on, and the documentation matches the dependency graph.

- Problem and users:
  - Scope: the promptforge repository on branch `master`.
    - Baseline `75245481`, which was `origin/master` when the debt analysis ran. Endpoint `9eac5f3b`, with a clean worktree; it has since been pushed, so `origin/master` is now `9eac5f3b`.
    - Target: the 14 commits of `vibe/2026-09-23-1-promptforge-api-firewall.md`, namely `cd2128cd`, `2ac1b540`, `e0c3b575`, `57f4cf6d`, `c6b38a04`, `49c858eb`, `871fd054`, `bc8df325`, `9963c477`, `799a1bbc`, `bf9d8859`, `a0d4c65d`, `2cee387a`, and `9eac5f3b`.
    - Every line number in this plan refers to `9eac5f3b`.
  - `chunk-names` (made worse by `cd2128cd`):
    - Three chunk-name constants point at a directory that no longer exists: `crates/promptforge-internal/lua/src/coro.rs` lines 31 and 39 (`TASKS_CHUNK_NAME`, `FANOUT_CHUNK_NAME`) and `crates/promptforge-internal/lua/src/messages.rs` line 28 (`MESSAGES_CHUNK_NAME`). Each reads `@crates/promptforge/lua/src/__impl_*.lua`, but `crates/promptforge/` is now the facade.
    - The sources they label are embedded by `include_str!` (coro.rs lines 36 and 45, messages.rs line 31) from `crates/promptforge-internal/lua/src/`. The constants' docs (coro.rs line 24, messages.rs line 26) promise a clickable `file:line` reference.
    - `cd2128cd` moved the files unchanged, and `2cee387a` repaired only `SHIM_CHUNK_NAME` (coro.rs line 25).
    - These literals were hand-edited in `a9a6c66c`, `7546bd9c`, `126e3b20`, and `2cee387a`, across four plans. Two of those edits set another wrong path.
    - Impact: developer tracebacks through the tasks, fanout, and messages chunks cite a file that does not exist.
  - `listing-fidelity` (introduced by `871fd054` and `bf9d8859`):
    - The renderer builds struct, union, enum, and variant lines from keyword, path, generics, where clause, and discriminant only. See `Renderer::line` around lines 62-76 and `Renderer::data` around lines 177-181 in `crates/build-xtask/src/api/render.rs`.
    - The renderer builds each line without reading `Item::attrs`, `has_stripped_fields`, or struct and variant kind, although `rustdoc-types` 0.61.0 provides all three. `crates/build-xtask/src/api/walk.rs` lines 16, 143-151, and 204-213 read the kinds, but only to enumerate fields.
    - As a result, `crates/promptforge/public-api.txt` renders `Op` (line 369, exhaustive) and `VfsError` (line 376, non-exhaustive) the same way. It also renders `AllowAll` (line 1466, a unit struct) and `ExecId` (line 1468, a private tuple field) the same way.
    - These edits all break hosts while `cargo xtask api --check` passes with no listing change:
      - adding `#[non_exhaustive]` or a private field to `AllowAll`
      - turning `Mode::Agent` into `Agent {}`
      - adding `#[non_exhaustive]` to `Op`
    - That contradicts `bf9d8859`'s stated purpose: every change to what hosts can reach should show up as a change to the listing.
  - `test-support` (made worse by `2ac1b540`, `e0c3b575`, and `57f4cf6d`):
    - `crates/promptforge/Cargo.toml` line 27 declares `test-support = ["promptforge-engine/test-support"]`. `crates/promptforge/src/lib.rs` lines 210-218 export a `test_support` module (`BoxFuture`, `Performer`, `Performers`, `drive_tokio`), documented in `crates/promptforge/src/test_support.md`.
    - The facade uses none of it, yet line 35 of its manifest gates its own test suite on the feature.
    - Consumers:
      - `crates/harness/capabilities/Cargo.toml` line 27, used from `crates/harness/capabilities/tests/it/support.rs` lines 15 and 78.
      - The engine's `drive_tokio` doc example (`crates/promptforge-internal/engine/src/test_support/tokio_driver.rs`, around line 84), reached through `crates/promptforge-internal/engine/Cargo.toml` line 53.
    - Because the feature exists, the surface check builds rustdoc twice: `report` in `crates/build-xtask/src/api.rs` (from line 166) iterates the `Build` enum in `crates/build-xtask/src/api/load.rs` (lines 42-71). It also makes `crates/build-xtask/src/facade_shape.rs` lines 31, 187, and 247-253 admit a cfg attribute on the facade.
    - The firewall plan deferred removal until after it landed (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 373), and it has now landed.
  - `doctest-cycle` (introduced by `799a1bbc`; notes restated by `2cee387a`):
    - The facade `promptforge` depends on every internal crate, and five of them list it back as a dev-dependency:
      - `crates/promptforge-internal/types/Cargo.toml` line 24
      - `crates/promptforge-internal/model-client/Cargo.toml` line 28
      - `crates/promptforge-internal/parser/Cargo.toml` line 28
      - `crates/promptforge-internal/store/Cargo.toml` line 23
      - `crates/promptforge-internal/engine/Cargo.toml` line 53
    - The only purpose is letting doc examples compile against facade paths: 77 `promptforge::` lines in doc comments across 21 `.rs` files under `crates/promptforge-internal/`, not counting the manifest comments, with no code import.
    - `AGENTS.md` line 35 says the dependency rules bind dev dependencies too. Six statements deny these edges:
      - `crates/promptforge-internal/engine/AGENTS.md` line 8
      - `crates/promptforge-internal/model-client/AGENTS.md` line 10
      - `crates/promptforge-internal/parser/AGENTS.md` line 6
      - `crates/promptforge-internal/store/AGENTS.md` line 5
      - `crates/promptforge-internal/types/AGENTS.md` line 7
      - `crates/promptforge-internal/README.md` line 11
    - The firewall plan logged the cycle as a medium risk (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 349) but never recorded an exception.
    - Impact: readers get a false picture of the dependency graph. Focused test builds of the leaf crates also compile the engine plus vendored Lua (inferred from the dependency graph, not measured).
- Goals:
  - `chunk-names`: every Lua chunk name renders the path of the source it embeds, and a test fails when one drifts.
  - `listing-fidelity`: the committed listing changes whenever a facade type's exhaustiveness, hidden-field status, or struct or variant kind changes.
  - `test-support`: the facade exports nothing that exists for tests - no `test-support` feature and no `test_support` module.
  - `doctest-cycle`: repository policy and crate notes describe the graph the manifests declare, with the doctest-only facade dev-dependencies recorded as an owned exception.
- Non-goals:
  - Moving internal doc examples into facade pages.
  - A structural check that the facade dev-dependency stays doctest-only.
  - Relocating the activation suite into `harness-sessions`.
  - Editing `vibe/2026-09-23-1-promptforge-api-firewall.md` or `vibe/archdoc.md`.
- Success criteria:
  - `rg -n "@crates/promptforge/lua" crates` returns nothing, and the chunk-name tests pass.
  - The re-blessed `crates/promptforge/public-api.txt` marks `VfsError` and every other non-exhaustive surface type and variant, and does not mark `Op` or `Verdict`. `AllowAll` renders as a unit struct, and `ExecId` renders with private fields. The set of listed items is unchanged.
  - `rg -n "test-support|test_support" crates/promptforge` returns nothing, no manifest enables `promptforge/test-support`, `cargo xtask api` performs one rustdoc build, and the harness-capabilities activation suite passes with its test count unchanged.
  - `AGENTS.md` states the doctest-only exception, and none of the six statements listed under `doctest-cycle` contradicts the manifests.
  - Every exit criterion in Testing Plan passes.
- Constraints:
  - No persisted, wire, or trust-boundary change.
  - No new structural check (source parser, snapshot, allowlist, count, ceiling, or topology check). The listing change refines the surface snapshot the user already approved (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 319).
  - The facade is `publish = false` (`crates/promptforge/Cargo.toml` line 7), so removing its feature affects only workspace crates.
  - Surface commands need the pinned toolchain `nightly-2026-09-05`, which pairs with `rustdoc-types` 0.61.0 (`crates/build-xtask/src/api/toolchain.rs` lines 16-19). On any other toolchain, `cargo xtask api` refuses to run.
- Open questions: None

## Functional Specification

Behavior changes only where the debts touch observable output. That means traceback text for three Lua chunks, the lines of the committed surface listing, the facade's feature set, and the crate notes contributors read. Every other behavior stays as it is: the facade's reachable items, the run engine, persisted logs, and the wire.

- Actors and workflows:
  - Engine developers read Lua tracebacks. A frame from the tasks, fanout, or messages chunk names a file that exists and holds that chunk's source.
  - Reviewers read `crates/promptforge/public-api.txt` diffs. A change to a facade type's exhaustiveness, hidden fields, or kind appears there.
  - Contributors and agents read crate notes and `AGENTS.md` before adding dependencies. What they read matches the manifests.
  - The harness-capabilities activation suite drives runs through a loop built only from items the facade exports for every host.
- Inputs and outputs:
  - In tracebacks, chunk names render as `@crates/promptforge-internal/lua/src/__impl_coro.lua` (shim), `__impl_tasks.lua`, `__impl_fanout.lua`, and `__impl_messages.lua` under that same directory.
  - `cargo +nightly-2026-09-05 xtask api --check` and `--bless` read one rustdoc JSON build of the facade with default features. They write or compare listing lines in this notation:
    - `#[non_exhaustive] ` prefixes a struct, union, enum, or variant line when the item carries the attribute.
    - Kind suffixes, placed after any generics and where clause: a unit struct ends in `;`, a unit variant has no suffix, a tuple kind ends in `(..)`, and a braced kind ends in ` { .. }`. A variant's ` = <discriminant>` stays as it is.
    - `..` means every field has its own listed line. `/* private fields */` replaces `..` when any field is hidden.
    - Examples: `#[non_exhaustive] pub enum promptforge::vfs::VfsError`, `pub struct promptforge::vfs::AllowAll;`, `pub struct promptforge::vfs::ExecId(/* private fields */)`.
- States and validation:
  - `--check` fails on any listing difference, including a difference in the new annotations (the prefix and the kind or private-field suffix).
  - `--bless` still refuses while any violation remains.
  - The facade shape check admits only doc attributes on facade items, so any `#[cfg(...)]` attribute is now a violation.
- Errors and recovery:
  - The activation suite's run loop panics, naming the effect, when a run issues anything but a store effect.
  - A chunk-name test failure names the constant whose path does not resolve, or whose file differs from the embedded source.
  - Cargo rejects any manifest that enables `promptforge/test-support`, because the feature no longer exists.
- Security and privacy behavior: No change. No trust boundary, credential, or persisted data is touched.
- Acceptance criteria:
  - A traceback through the tasks, fanout, or messages chunk cites a real file under `crates/promptforge-internal/lua/src/`.
  - Each of these edits to a blessed facade type makes `--check` fail: adding `#[non_exhaustive]`, adding a private field, or changing its struct or variant kind.
  - The facade builds, documents, and tests with no features, and has no feature named `test-support`.
  - The six crate statements and `AGENTS.md` line 35 agree with `cargo tree --workspace -e dev -i promptforge --locked --depth 1`.

</product-contract>
<implementation-contract>

## Technical Design

The four fixes are independent and touch different components. They overlap in only four shared files: `AGENTS.md`, `crates/promptforge-internal/engine/Cargo.toml`, `crates/build-xtask/src/api.rs`, and `crates/build-xtask/src/api/listing-tests.rs`. The facade's public interface shrinks: one Cargo feature and one feature-gated module go away, and nothing is added. `build-xtask`'s surface check drops to one build and renders three more facts per type line. The dependency graph keeps its doctest-only upward dev edges, which repository policy now names as an exception.

- Architecture:
  - Graph: the five internal crates keep their `promptforge` dev-dependency for doctests only, but the engine's entry no longer enables `test-support`. `harness-capabilities` loses its facade dev-dependency; its normal `promptforge` dependency on `crates/harness/capabilities/Cargo.toml` line 19 stays.
  - The engine keeps its own `test_support` module, gated by `#[cfg(any(test, feature = "test-support"))]` (`crates/promptforge-internal/engine/src/lib.rs` line 13). The `models_loop` bench keeps its own `required-features = ["test-support"]`.
- Modules and interfaces:
  - `promptforge` (the facade) exports exactly the items in its re-blessed listing, all of them reachable with default features. Its `[features]` table disappears if it ends up empty.
  - `harness-capabilities` activation suite: runs are driven through `Run::step`, `Run::resume`, `Step::Pending`, `Step::Done`, `Effect::Store`, `EffectAnswer::Store`, and `vfs::perform_store_op`, all already exported. `crates/promptforge/src/lib.md` lines 44-73 show the same loop.
  - `build-xtask` `api` module: one rustdoc build with default features, so findings carry no per-build label. The listing notation is the one in Functional Specification, documented in the module docs of `crates/build-xtask/src/api.rs`.
  - `build-xtask` facade shape check: facade items may carry only doc attributes, alongside the existing rules on modules and single-item `pub use`.
  - Repository policy: `AGENTS.md` line 35 gains this exception text: "One exception: a crate under crates/promptforge-internal/ may list `promptforge` in `[dev-dependencies]` only so its doc examples compile against the facade paths hosts see. No unit test, integration test, or bench imports it. This edge is exempt from the one-way flow rule under Structural Rules." The six statements listed under `doctest-cycle` each name that exception and keep their meaning for every other dependency.
- File and public API changes:
  - Removed from the facade's public API: the Cargo feature `test-support`, the module `promptforge::test_support` (`BoxFuture`, `Performer`, `Performers`, `drive_tokio`), and `crates/promptforge/src/test_support.md`. None of these appear in the committed listing today, because the listing covers only the default build.
  - `crates/promptforge/public-api.txt` is re-blessed. Only the new annotations change and the set of listed items stays the same, but the diff is large. Nearly every struct, union, and variant line gains a kind suffix, and non-exhaustive lines move into one block at the top of the file.
  - The chunk-name strings in `crates/promptforge-internal/lua/src/coro.rs` lines 31 and 39 and `crates/promptforge-internal/lua/src/messages.rs` line 28 become `@crates/promptforge-internal/lua/src/__impl_tasks.lua`, `__impl_fanout.lua`, and `__impl_messages.lua`.
- Data, persistence, failure, security, and privacy constraints:
  - No persisted, wire, or trust-boundary change.
  - Traceback text changes for three chunks. No recorded run logs exist (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 308), and no test matches the old strings.

</implementation-contract>
<verification-contract>

## Testing Plan

Each debt gets a check that fails while the debt exists and passes once it is gone. Chunk names get content-match unit tests. The listing gets pinned-nightly fixtures, including a check-mode fixture proving that an annotation change fails `--check`. The test-support removal is proven by the activation suite passing unchanged on the new loop, by the facade suite running with default features, and by a single-build surface check. The dependency exception is a review check against `cargo tree`, and the whole workspace then passes every repository gate.

- Unit:
  - `chunk-names`:
    - A test in promptforge-lua covers each of the four chunk-name constants (shim, tasks, fanout, messages). It strips the leading `@`, resolves the path against the workspace root (`env!("CARGO_MANIFEST_DIR")` joined with `../../..`), and asserts that the file's contents equal the embedded source.
    - Temporarily pointing one constant at a wrong directory makes its test fail. Revert after checking.
    - Command: `cargo nextest run --locked -p promptforge-lua`.
  - `test-support`: a `crates/build-xtask/src/facade_shape-tests.rs` case asserts that a `#[cfg(feature = "test-support")]` facade item is now a violation. Command: `cargo test -p build-xtask`.
  - `listing-fidelity`:
    - New fixtures marked `#[ignore = "needs the pinned nightly"]` go in `crates/build-xtask/src/api/listing-tests.rs`. They assert the exact lines for:
      - an exhaustive enum and a non-exhaustive enum
      - a unit struct
      - a tuple struct with a private field
      - braced structs with and without private fields
      - unit, tuple, and struct variants, one of them non-exhaustive
    - Command: `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Integration and end-to-end:
  - `test-support`: `cargo nextest run --locked -p harness-capabilities` passes with the same test count as before the change. `cargo test -p promptforge` with default features builds and runs the facade suite. `cargo test -p promptforge-engine --all-features --doc` passes the `drive_tokio` example.
  - `listing-fidelity`: `cargo +nightly-2026-09-05 xtask api --bless`, then `cargo +nightly-2026-09-05 xtask api --check`, passes with one build. In the listing diff, only the new annotations change and the set of listed items is unchanged.
  - `doctest-cycle`: `cargo tree --workspace -e dev -i promptforge --locked --depth 1` lists the facade's direct dev-dependents.
    - Among them, the crates under `crates/promptforge-internal/` are exactly types, model-client, parser, store, and engine.
    - The harness crates it also lists are outside this check: `harness-log` stays, and `harness-capabilities` disappears once the test-support change removes its edge.
    - Each of the six statements, and `AGENTS.md` line 35, agrees with that output.
- Regression, security, and performance:
  - `listing-fidelity`: a check-mode fixture blesses a fixture facade, adds `#[non_exhaustive]` to a unit struct in it, and asserts that `--check` reports a difference.
  - `chunk-names`: `rg -n "@crates/promptforge/lua" crates` returns nothing.
  - `test-support`: `rg -n "test-support|test_support" crates/promptforge` returns nothing. The check covers only the facade crate; fixtures under `crates/build-xtask/` keep the string on purpose.
  - No security or performance criterion. Build time is not measured.
- Exit criteria:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`
  - `mdbook build guide`
  - `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - `doctest-cycle`:
    - The call: record a doctest-only exception in `AGENTS.md` and correct the six statements. The five dev-dependencies and the per-item examples stay.
    - Rationale: the harm shown so far is false documentation, which this fully fixes. Moving 77 examples would strip per-item examples from host-facing facade pages, and the build cost is unmeasured.
    - The user's words, choosing between moving the examples and recording the exception: "I think B for the first one".
  - `test-support`:
    - The call: remove the facade feature and module now, and drive the activation suite with a test-local host loop.
    - Rationale: removing the feature leaves zero test surface on the facade, whether the tests use a local loop or move crates. The local loop changes one function, keeps the tests in their crate, and only has to answer store effects.
    - The user's words: "for the second one which choice minimizes the public API surface of the promptforge facade?"
    - Earlier user words recorded in `vibe/2026-09-23-1-promptforge-api-firewall.md`: "I want ZERO public API surface growth just for tests." (line 303), and the deferral "no. we will deal with it later." (line 373). That deferral's revisit condition, "after this plan lands", has been met.
  - `chunk-names`:
    - The call: treat chunk names as file paths, overriding the firewall plan's "labels, not file paths" instruction (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 818).
    - Rationale: the constants' own docs call them file paths, `2cee387a` already fixed the shim as a path, and no test or recorded log depends on the old text. The content-match test checks the documented diagnostic promise, not implementation shape.
    - The user's words: none. This is a reversible call made during debt analysis.
  - `listing-fidelity`:
    - The call: mark types with a `#[non_exhaustive]` prefix, plus the kind and private-field suffixes defined in Functional Specification.
    - Rationale: this matches the rustdoc and `cargo-public-api` syntax reviewers already read, and it refines the approved snapshot instead of adding a check.
    - The user's words approving the snapshot (`vibe/2026-09-23-1-promptforge-api-firewall.md` line 319): "everything you recommend, based on the constrains and direcetion I want to go implied in our chat".
  - Commit granularity:
    - The call: one commit per debt, so four steps. The test-support consumers, the facade removal, and the single-build surface check land together.
    - Rationale: every step carries a fixed cost in coding, review, fix, verify, and commit-message rounds, so fewer steps finish sooner. The test-support parts can't land separately in any useful way, because the feature can't go before its consumers.
    - The user's words: "can we make the steps go more quickly", then choosing "One commit per debt: fold Steps 2-4 into a single test-support step, so 6 steps become 4". Those were the three test-support steps of an earlier six-step decomposition, which are now Step 2.
  - Activation loop failure mode:
    - The call: the loop panics on any non-store effect instead of refusing it.
    - Rationale: a future fixture that needs a chat, tool, or input performer then fails loudly instead of quietly changing behavior.
    - The user's words: none. Reversible.
- Rejected alternatives:
  - Moving the internal doc examples into facade module and topic pages and dropping the five dev-dependencies.
    - Reason: facade item pages would lose their per-item examples.
    - Revisit: when focused test build time for the leaf crates is measured and matters.
  - Keeping the facade `test-support` feature deferred.
    - Reason: it leaves test surface on the public crate after its own revisit condition was met.
    - Revisit: none.
  - Moving the activation cases into `harness-sessions` and driving them through the production loop.
    - Reason: the surface result is the same, but it moves test ownership and is a bigger change.
    - Revisit: when an activation fixture needs chat, tool, or input performers.
  - Deriving chunk names from `file!()` or `CARGO_MANIFEST_DIR`.
    - Reason: it would put machine- or build-dependent text into deterministic error output.
    - Revisit: if the lua crate is ever built outside this workspace layout.
  - A shared directory constant with no test.
    - Reason: a directory move would still strand the constant silently.
    - Revisit: when more embedded chunks are added; it can then sit alongside the test.
  - Suffix placement for the `#[non_exhaustive]` marker.
    - Reason: it keeps items next to their sort neighbors but reads unlike Rust.
    - Revisit: if the non-exhaustive block at the top of the listing hampers review.
  - Rendering `Enum::has_stripped_variants`.
    - Reason: a variant is always as visible as its enum, so it can only be stripped by `#[doc(hidden)]`. The `doc(hidden)` ban in `crates/build-xtask/src/doc_hidden.rs` rejects that in the facade and the container.
    - Revisit: if that ban is lifted.
  - Per-type compile-time assertions for exhaustiveness and constructibility.
    - Reason: they do not scale to about 80 surface structs plus the enums.
    - Revisit: if the surface listing is retired.
  - A structural guard that the facade dev-dependency stays doctest-only.
    - Reason: the user accepted the exception without a check, and structural checks need explicit approval.
    - Revisit: if a non-doctest import of `promptforge` appears in a crate under `crates/promptforge-internal/`.
- Assumptions, risks, and notes:
  - Line numbers refer to `9eac5f3b`, and each step's are exact when that step starts. The exception is a later step citing a file an earlier step already edited. The shared files are `AGENTS.md` (Step 2 edits lines 55-56, and Step 3 edits line 35 above them, so line 35 holds), `crates/promptforge-internal/engine/Cargo.toml`, `crates/build-xtask/src/api.rs`, and `crates/build-xtask/src/api/listing-tests.rs`. In those files, locate the named text or symbol instead of trusting the line number.
  - `nightly-2026-09-05` is installed on the development machine, and `rustdoc-types` 0.61.0 is the newest release on crates.io (published 2026-07-29). The pinned pair is therefore current, and bumping it is out of scope.
  - The activation suite only raises store effects.
    - Three of the four `run_activated` calls return a refusal before the run is driven: `crates/harness/capabilities/tests/it/activation.rs` around line 194, and `crates/harness/capabilities/tests/it/assembly.rs` around lines 149 and 341.
    - The fourth (`activation.rs` around line 220) runs a prompt whose only effect is `store.read('activated.txt')`.
    - `crates/harness/capabilities/tests/it/support.rs` lines 61-65 state that no fixture performs a chat, tool, or input effect.
    - A future fixture that needs more adds a branch to the test loop, never a facade export.
  - The pinned nightly's rustdoc JSON is assumed to populate `Attribute::NonExhaustive` and `has_stripped_fields` as `rustdoc-types` 0.61.0 declares them. The new fixtures prove this before the re-bless.
  - Accepted consequences of the doctest exception: leaf-crate test builds still compile the engine and vendored Lua. No check catches a future upward dev dependency or a non-doctest use of the facade inside the container.
  - The engine's `drive_tokio` example ends up mixing `promptforge::` imports with `promptforge_engine::test_support` imports. They name the same types, because the facade re-exports engine items.
  - The debt analysis behind this plan was static; no builds or tests were run. It rejected 60 candidates:
    - 23 residual but acceptable: user-approved plan choices, private `build-xtask` constants, test-only fixture duplication, and stale comments.
    - 16 weak or speculative: no demonstrated consequence.
    - 17 false: disproved.
    - 4 unrelated and pre-existing.
  - No exposed pre-existing debt survived.

### Deferred and Out of Scope

- Deferred: None
- Out of scope:
  - `Prompt::strip_h1_prose`, which has been public with no caller since before the baseline.
  - The stale `RUSTDOC_FLAGS` comment in `crates/build-xtask/src/api/load.rs` lines 27-29.
  - The stale "hidden seams" sentence in `crates/promptforge-internal/store/AGENTS.md` line 6.
  - Switching `crates/build-xtask/src/api/load.rs` from `engine_guards::collect_crates` to `product::workspace_crates`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace default member; plain `cargo build` builds only the gateway). Desktop app: `cargo build --locked -p workshop`, after staging the gateway sidecar with `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source <gateway exe>`; the full desktop release flow is the `cargo workshop` alias. The workshop-server and gateway-config-ui build scripts bundle their `ui/` with esbuild, so run `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui` before any cargo build.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`; drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Integration tests are one binary per crate, so add `--test it` (the `promptforge` facade uses `--test suite`). One crate's doctests: `cargo test --locked -p <crate> --all-features --doc`. Gateway process-race tests: `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <test-name>`. One UI test file: `node --test <path>` from the UI package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` then `cargo test --locked -p <crate> --all-features --doc` (workshop trio without `--all-features`; `workshop-server` also runs `cargo nextest run --locked -p workshop-server --features headless`). Any change touching crate boundaries, manifests, lib.rs markers, or file sizes also runs the structural harness `cargo test -p build-xtask`. UI packages: `npm test --prefix crates/workshop/ui` and `npm test --prefix crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. The facade surface gate runs on the pinned nightly named in `crates/build-xtask/src/api/toolchain.rs` (currently `nightly-2026-09-05`): `cargo +nightly-2026-09-05 xtask api --check` and `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`; it compares against the committed `crates/promptforge/public-api.txt`. UI: `npm test` in each UI package.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless feature gate `cargo check -p gateway --no-default-features` (the only permitted standalone `cargo check`; never run `cargo check --workspace`). UI: `npm run typecheck` in each UI package. Supply chain: `cargo deny check` and `cargo audit`.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook). No TypeScript formatter is configured.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS="-D warnings"`), then facade docs `cargo doc -p promptforge --no-deps` under the same flag and without `--all-features`; user guide: `mdbook build guide`.
- Test placement and naming conventions: Unit tests mostly live in a sibling file `<module>-tests.rs` wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (about 150 such files); small modules use an inline `#[cfg(test)] mod tests { ... }`; directory modules may hold a `tests/` subdirectory (for example `engine/src/execute/tests/`) or `tests-<topic>.rs` splits. Integration tests use one binary per crate at `tests/it/main.rs` pulling in topic files (the `promptforge` facade uses `tests/suite/`), with prompt fixtures under `tests/prompts/`. Test functions are descriptive snake_case sentences (for example `a_direct_launch_recovers_the_lease_from_a_terminated_owner`). UI tests are `*.test.mjs` beside source in `src/` or under `test/`, run by `node --test`. `.config/nextest.toml` puts the STT FFI suites in a throttled `heavy` group.
- Directory map:
  - `crates/` root: the public and shared layer. `promptforge` (engine facade with committed `public-api.txt`), `harness-api`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS package, not a Rust crate), `workspace-hack` (cargo-hakari), and build tooling `build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`.
  - `crates/promptforge-internal/`: private engine family (types, engine, lua, parser, store, vfs, model-client).
  - `crates/harness/`: private harness family (runner, models, capabilities, log, sessions, web, webfetch, web-search).
  - `crates/gateway/`: private gateway family (app, cloud-providers, config, config-ui with its `ui/` SPA, local, logging, progress, protocol, routing, web-search, and the nested `stt/` subsystem: api, engine, backend-whisper, whisper-ffi).
  - `crates/workshop/`: private workshop family (shell, the Tauri app package `workshop`; server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace; and the `ui/` SPA).
  - `guide/`: mdBook user guide sources plus per-product guides.
  - `prompts/`: sample prompt pipelines.
  - `tools/`: Node scripts for gateway sidecar staging and live TTS checks, with their tests.
  - `vibe/`: architecture doc (`archdoc.md`), plans, and dated run logs.
  - `local/`: developer-local gateway and MCP configs, profiles, and fixtures.
  - `.github/workflows/`: CI, nightly, and release pipelines. `.githooks/`: pre-commit format check; pre-push headless gateway check, clippy, and cargo deny. `.config/`: nextest and hakari. `.cargo/config.toml`: `workshop` and `xtask` aliases and rust-lld with static CRT on Windows.
  - `target/`, `target-msrv/`: build output.
- Component boundaries:
  - `promptforge` is the only crate outside the engine family allowed to depend into `crates/promptforge-internal/`. Inside, the executor is sans-I/O and depends on store, the Lua VM boundary, and shared substrate; store depends on vfs; vfs depends on nothing. promptforge crates never depend on gateway, workshop, or harness crates.
  - `harness-api` is the single public entry into `crates/harness/`. Harness crates may depend on `promptforge`, `gateway-api-types`, `gateway-api-discovery`, and shared-* crates, never on workshop crates or private gateway crates.
  - The gateway family exposes only `gateway-api-types` and `gateway-api-discovery` and depends only on shared-* crates, never on promptforge, harness, or workshop crates.
  - Workshop crates may depend on `promptforge`, `harness-api`, the gateway public pair, and shared-* crates. The `workshop` shell depends on `workshop-server-api`, never on `workshop-server`.
  - shared-* crates depend on no product crate. A crate inside a family container may depend only on `crates/` root crates and its own siblings. Tiers flow one way: shell, features, services, vocabulary. These rules bind normal, dev, build, and target-specific dependencies, and `cargo test -p build-xtask` plus `cargo xtask api --check` enforce them.
- Conventions summary:
  - Rust 2024 on the stable toolchain, resolver 3, `--locked` in CI. Every member inherits workspace lints and depends on `workspace-hack`.
  - Workspace lints: `unsafe_code = "forbid"` (unsafe only inside its owned boundary, with safety comments right before each block), clippy `all` and `pedantic` denied, `unwrap_used` and `expect_used` denied, `missing_docs` and `unreachable_pub` warned, broken or private intra-doc links denied.
  - Source directories stay flat: one or two related files are kebab siblings `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`; three or more become a `foo/` subdirectory.
  - Every workshop-* and harness-* lib.rs opens with a `//!` doc holding a `## Invariants` marker listing allowed and forbidden dependencies (the Tauri shell is exempt); no Rust file in those crates exceeds 500 lines.
  - Errors are typed per crate (thiserror) and wrap third-party causes through `shared-error-source`; error and status messages are written for model consumption, naming required versus actual.
  - Behavior changes ship with tests in the same change. No new structural checks (parsers, allowlists, counts, topology walkers) without explicit user approval. Cargo features gate real constraints only.
  - Comments explain non-obvious constraints only; workarounds cite an upstream issue URL. Root `Cargo.toml` documents the reason behind each version pin.
  - JSON reaching the run log or replay round-trips exactly: canonical sorted keys, `float_roundtrip`, never `preserve_order`.
  - SPA: CSS sits beside its TypeScript per feature directory, uses only `--ws-*` tokens, and never touches `localStorage`; persisted UI state goes through the `ui-storage` adapter to the server.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Repoint the Lua chunk names and pin them with content-match tests [completed]

- Component: chunk-names
- Component placement: first. It depends on nothing and edits no file another component touches, so it lands cleanly on `9eac5f3b` with every line number exact.
- Piece: chunk-name paths, built jointly. The constants and the content-match tests are one behavior: the tests fail until the constants are repointed, and nothing else checks the constants.
- Constants:
  - `crates/promptforge-internal/lua/src/coro.rs` line 31 `TASKS_CHUNK_NAME` becomes `@crates/promptforge-internal/lua/src/__impl_tasks.lua`, and line 39 `FANOUT_CHUNK_NAME` becomes `@crates/promptforge-internal/lua/src/__impl_fanout.lua`.
  - `crates/promptforge-internal/lua/src/messages.rs` line 28 `MESSAGES_CHUNK_NAME` becomes `@crates/promptforge-internal/lua/src/__impl_messages.lua`.
  - `SHIM_CHUNK_NAME` (coro.rs line 25) is already correct and only gains a test.
- Shared helper: a `pub(crate)` function in the crate-level test module `crates/promptforge-internal/lua/src/tests.rs` (declared `#[cfg(test)] mod tests;` in `lib.rs` lines 146-147). It takes the constant's name, the chunk name, and the embedded source. It strips the leading `@`, resolves the rest against `env!("CARGO_MANIFEST_DIR")` joined with `../../..`, and asserts that the file's contents equal the embedded source. Its failure message names the constant, whether the path does not resolve or the file differs.
- Callers, one test function per pair:
  - A new `crates/promptforge-internal/lua/src/coro-tests.rs`, wired into `coro.rs` as `#[cfg(test)] #[path = "coro-tests.rs"] mod tests;`, covers `SHIM_CHUNK_NAME` with `SHIM_SOURCE`, `TASKS_CHUNK_NAME` with `TASKS_SOURCE`, and `FANOUT_CHUNK_NAME` with `FANOUT_SOURCE`.
  - The existing `crates/promptforge-internal/lua/src/messages-tests.rs` (wired at `messages.rs` lines 55-57) covers `MESSAGES_CHUNK_NAME` with `MESSAGES_SOURCE`.
- Verification:
  - `cargo nextest run --locked -p promptforge-lua` passes.
  - Temporarily point one constant at a wrong directory, confirm its test fails and names that constant, then revert.
  - `rg -n "@crates/promptforge/lua" crates` returns nothing.
- Commit: the three constants, the helper, `coro-tests.rs` with its `coro.rs` wiring, and the `messages-tests.rs` test.

</step-1>

<step-2>

### Step 2: Remove the facade's test-support feature and module, move its two consumers off it, and collapse the surface check to one build [completed]

- Component: test-support
- Component placement: second. It is the only component that changes the dependency graph, so `doctest-cycle` follows it and reviews the final graph. Its collapse edits to `crates/build-xtask/src/api.rs`, `crates/build-xtask/src/api/listing-tests.rs`, and `crates/build-xtask/src/api/fixture-test-support.rs` (sites listed below) land before `listing-fidelity` adds to those files. That keeps Step 2's line numbers exact, and the new listing fixtures get written once, against the single-build loader.
- Pieces: two consumer pieces and one removal piece, built in that order within one commit. The removal comes last, because deleting the feature breaks any consumer still using it. The removal's own parts also break each other when split: `cargo xtask api` passes `test-support` to its second rustdoc build, which fails once the feature is gone, and the facade shape check's cfg allowance exists only for the module being deleted.
- Before editing anything, record the test count from `cargo nextest run --locked -p harness-capabilities`.
- Consumer piece 1, the harness-capabilities activation suite. `crates/harness/capabilities/tests/it/support.rs`:
  - Add a private synchronous store-only loop over `Run::step` and `Run::resume`. On `Step::Pending` holding `Effect::Store`, it answers with `EffectAnswer::Store` from `vfs::perform_store_op`. On `Step::Done` it returns the `RunResult`. On any other effect it panics, naming the effect. `crates/promptforge/src/lib.md` lines 44-73 show the same loop.
  - Replace the `drive_tokio` call on line 78 with that loop, and make `run_activated` synchronous.
  - Remove the `promptforge::test_support` import on line 15. Rewrite the doc comment on lines 61-65 to describe the store-only loop and its panic instead of the engine's tokio driver.
- Call sites: drop `.await` at the four `run_activated` calls, in `crates/harness/capabilities/tests/it/activation.rs` around lines 194 and 220 and in `crates/harness/capabilities/tests/it/assembly.rs` around lines 149 and 341. Keep `#[tokio::test]` only where a test still awaits, and make the rest `#[test]`.
- `crates/harness/capabilities/Cargo.toml`: remove the facade dev-dependency on line 27, which enables `promptforge/test-support`, plus any dev-dependency that becomes unused. The normal `promptforge` dependency on line 19 stays.
- Consumer piece 2, the engine's `drive_tokio` example. The two consumer pieces touch different crates and can be built in either order.
- `crates/promptforge-internal/engine/src/test_support/tokio_driver.rs`: the `drive_tokio` doc example (around line 84) imports `Performers` and `drive_tokio` from `promptforge_engine::test_support` instead of the facade. Its other `promptforge::` imports stay; they name the same types because the facade re-exports engine items.
- `crates/promptforge-internal/engine/Cargo.toml`: the `promptforge` dev-dependency on line 53 drops `features = ["test-support"]`, leaving `promptforge.workspace = true`. The comment on lines 51-52 drops its last sentence ("`test-support` for the `test_support::drive_tokio` example."), so it gives only the doctest reason.
- Unchanged: the engine's own `test_support` module and its `#[cfg(any(test, feature = "test-support"))]` gate (`crates/promptforge-internal/engine/src/lib.rs` line 13), the engine's `test-support` feature, and the `models_loop` bench's `required-features = ["test-support"]`.
- Removal piece, built after both consumer pieces. `crates/promptforge/`:
  - `Cargo.toml`: remove the `test-support` feature on line 27 (and the `[features]` table if it ends up empty) and `required-features` on line 35, so the facade suite runs with default features.
  - `src/lib.rs`: remove the `test_support` module on lines 210-218.
  - Delete `src/test_support.md`, and remove the sentence on `src/lib.md` line 34.
- `crates/build-xtask/`:
  - Collapse to one build by deleting the `Build` type outright, not by keeping a one-variant enum. `report` takes only the root, loads once, always produces the listing, and prints findings without a per-build label. The exact sites:
    - `src/api/load.rs`:
      - Delete the `Build` enum with its doc comment and derives, and both of its impls (lines 42-71).
      - `load(root, build)` on line 115 becomes `load(root)`. Line 120 stops joining a per-build directory, so the JSON lands in `target/xtask-api`.
      - `run_cargo_doc` drops its `build` parameter (lines 121 and 149) and the `command.args(build.features())` call (line 161). Its error messages on lines 164 and 171 drop the `{build} build: ` prefix.
      - Reword the doc comments on lines 2, 73, and 114 that describe a per-build load.
    - `src/api.rs`:
      - Reword the module docs on lines 4-6 so they describe one default-features build, and remove `use load::Build` on line 33.
      - `Report` (lines 74-78) holds `findings: BTreeSet<Finding>`, and its doc comment no longer mentions builds.
      - `execute` calls `report(root)` (line 117) and formats each finding with no `[builds]` label (lines 126-133).
      - `report` (from line 166) loses its `builds` parameter and loop. It sets the listing unconditionally, so the `build == Build::Default` test on line 175 goes away.
    - `src/api/load-tests.rs`: delete `the_test_support_build_is_closure_checked_and_left_out_of_the_listing` (lines 63-103), which exercises the removed second build. Drop the module doc's description of that build (lines 1-3).
    - `src/api/listing-tests.rs`: drop the import on line 7, call `report(root.path())` on line 139, and drop the `[default, test-support] ` label from the expected text on line 146.
    - `src/api/listing-compact-tests.rs`: drop the import on line 9, and call `report(root.path())` on line 329.
    - `src/api/fixture-test-support.rs`: drop the import on line 12, and call `super::report(root)` on line 105. The synthetic `[features]` entries on lines 19 and 27 stay; without a second build they are inert.
    - Remove any import that becomes unused, such as `fmt` in `load.rs` and `BTreeMap` in `api.rs`.
  - `src/facade_shape.rs` lines 4, 31, 187, and 247-253: remove `is_test_support_cfg`, so facade items may have only doc attributes.
  - `src/facade_shape-tests.rs`:
    - Remove the `#[cfg(feature = "test-support")]` attributes, and the items they gate, from the accepted fixture at lines 106-108.
    - Add a `#[cfg(feature = "test-support")]` case to the table in the test at line 233, as a violation.
    - Rename that test from `attributes_other_than_doc_and_the_test_support_cfg_are_rejected` to `attributes_other_than_doc_are_rejected`. Its `cfg(not(feature = "test-support"))` case at line 246 stays a violation.
    - Update any expected text that quotes the old attribute rule.
  - `src/test_support_leak.rs` module docs:
    - On lines 9 and 20, the examples that name `promptforge` with `test-support` switch to `promptforge-engine`, which still has the feature.
    - On lines 24-25, the exemption's parenthetical stops presenting the facade's forwarding as current. It names a container crate forwarding a sibling's, and notes that the guard counts the facade as an engine crate too.
    - The guard and its fixtures stay unchanged. That includes `the_facade_forwarding_the_engine_test_support_feature_passes` (`src/test_support_leak-tests.rs` lines 211-226) and its twin in `src/engine_guards-tests.rs` lines 126-131. Both check a shape the guard still permits, which the reworded docs now describe.
  - Out of scope in `load.rs`: the stale `RUSTDOC_FLAGS` comment on lines 27-29 and the `engine_guards::collect_crates` call.
- `.github/workflows/ci.yml` line 144: the comment above the Facade docs job says the topic docs "must stand without the test-support module"; fix it so it states only that the facade docs build with default features.
- `AGENTS.md` line 55 now reads "so the facade's docs build with default features", and line 56 drops "with and without `test-support`".
- Verification:
  - `cargo nextest run --locked -p harness-capabilities` passes with the recorded test count.
  - `cargo test -p promptforge-engine --all-features --doc` passes, including the `drive_tokio` example.
  - `cargo test -p build-xtask` passes, including the new `facade_shape-tests.rs` violation case.
  - `cargo test -p promptforge` builds and runs the facade suite with default features.
  - `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only` passes. The nightly-only fixtures in `load-tests.rs`, `listing-tests.rs`, and `listing-compact-tests.rs` call the new `report` signature.
  - `cargo +nightly-2026-09-05 xtask api --check` passes with one rustdoc build and no listing change.
  - `rg -n "test-support|test_support" crates/promptforge` returns nothing. The check covers only the facade crate; fixtures under `crates/build-xtask/` keep the string on purpose. The workspace still resolves, which proves no manifest enables `promptforge/test-support`, because Cargo rejects a missing feature.
- Commit: `support.rs`, `activation.rs`, `assembly.rs`, and the harness-capabilities manifest; `tokio_driver.rs` and the engine manifest; the facade files, the build-xtask files, `ci.yml`, and `AGENTS.md`.

</step-2>

<step-3>

### Step 3: Record the doctest-only facade dev-dependency exception and correct the six crate statements [completed]

- Component: doctest-cycle
- Component placement: third, after `test-support`. Step 2 gave the graph its end state, so the `cargo tree` review here checks the final graph. It edits `AGENTS.md` line 35, above lines 55-56 that Step 2 changed in place, so line numbers stay exact.
- Piece: policy and notes, built jointly. The exception text and the six statements must agree with each other and with one `cargo tree` output, so they land together.
- `AGENTS.md` line 35 gains this text verbatim: "One exception: a crate under crates/promptforge-internal/ may list `promptforge` in `[dev-dependencies]` only so its doc examples compile against the facade paths hosts see. No unit test, integration test, or bench imports it. This edge is exempt from the one-way flow rule under Structural Rules."
- Amend each of these statements so it names that exception and keeps its meaning for every other dependency:
  - `crates/promptforge-internal/engine/AGENTS.md` line 8
  - `crates/promptforge-internal/model-client/AGENTS.md` line 10
  - `crates/promptforge-internal/parser/AGENTS.md` line 6
  - `crates/promptforge-internal/store/AGENTS.md` line 5
  - `crates/promptforge-internal/types/AGENTS.md` line 7
  - `crates/promptforge-internal/README.md` line 11
- Confirm that the `promptforge` dev-dependency in `crates/promptforge-internal/engine/Cargo.toml` (line 53 at `9eac5f3b`, shifted after Step 2 shortened its comment) reads `promptforge.workspace = true` under a comment that gives only the doctest reason.
- Unchanged: the five `promptforge` dev-dependencies (types line 24, model-client line 28, parser line 28, store line 23, engine line 53) and the per-item doc examples. Do not edit `vibe/2026-09-23-1-promptforge-api-firewall.md`, `vibe/archdoc.md`, or the out-of-scope "hidden seams" sentence on `crates/promptforge-internal/store/AGENTS.md` line 6. Add no structural check that the dev-dependency stays doctest-only.
- Verification: run `cargo tree --workspace -e dev -i promptforge --locked --depth 1`. The crates under `crates/promptforge-internal/` in its output are exactly types, model-client, parser, store, and engine. It also lists `harness-log`, which is outside this check; `harness-capabilities` no longer appears after Step 2. Each of the six statements and `AGENTS.md` line 35 agrees with that output.
- Commit: `AGENTS.md`, the five crate `AGENTS.md` files, and `crates/promptforge-internal/README.md`.

</step-3>

<step-4>

### Step 4: Render exhaustiveness, kind, and private-field markers in the surface listing, re-bless it, and run the exit gates

- Component: listing-fidelity
- Component placement: last. It depends on no other component, since the listing covers only the default build. It follows `test-support` so its fixtures are written once against the single-build loader, after Step 2's edits to `listing-tests.rs` and `api.rs`. Those two files have shifted by then, so locate their content by name rather than by `9eac5f3b` line number. It changes the committed listing, the last surface artifact, so the plan's exit gates run here on the finished tree.
- Piece: listing notation, built sequentially inside the step: renderer, then fixtures, then re-bless. They share one commit, because a renderer change without the re-blessed listing fails `cargo xtask api --check`. This refines the approved surface snapshot; add no new structural check.
- `crates/build-xtask/src/api/render.rs` (`Renderer::line` around lines 62-76 and `Renderer::data` around lines 177-181) renders:
  - the `#[non_exhaustive] ` prefix on a struct, union, enum, or variant line, from `Item::attrs`
  - the kind suffix after any generics and where clause, from the struct kind, the variant kind, or a union: `;` for a unit struct, nothing for a unit variant, `(..)` for a tuple kind, and ` { .. }` for a braced kind. A variant's ` = <discriminant>` stays as it is.
  - `/* private fields */` in place of `..` when any field is hidden. `rustdoc-types` 0.61.0 signals that in four places:
    - `Union.has_stripped_fields`, at the top level of the union
    - `StructKind::Plain { fields, has_stripped_fields }`
    - `VariantKind::Struct { fields, has_stripped_fields }`
    - a `None` entry in a tuple kind's `Vec<Option<Id>>`
  - `#[non_exhaustive]` on a variant comes from the variant item's own `attrs` (`visit.item.attrs`), not from its enum's.
- Document the notation in the module docs of `crates/build-xtask/src/api.rs`, with the examples `#[non_exhaustive] pub enum promptforge::vfs::VfsError`, `pub struct promptforge::vfs::AllowAll;`, and `pub struct promptforge::vfs::ExecId(/* private fields */)`.
- Fixtures in `crates/build-xtask/src/api/listing-tests.rs`, each marked `#[ignore = "needs the pinned nightly"]`:
  - Line fixtures asserting the exact lines for an exhaustive and a non-exhaustive enum, a unit struct, a tuple struct with a private field, braced structs with and without private fields, and unit, tuple, and struct variants with one of them non-exhaustive.
  - A check-mode fixture that blesses a fixture facade, adds `#[non_exhaustive]` to a unit struct in it, and asserts that `--check` reports a difference.
- Run the fixtures before re-blessing, with `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`. They prove the pinned nightly populates `Attribute::NonExhaustive` and `has_stripped_fields` as `rustdoc-types` 0.61.0 declares.
- Re-bless with `cargo +nightly-2026-09-05 xtask api --bless`, then confirm `cargo +nightly-2026-09-05 xtask api --check` passes with one build. In the diff of `crates/promptforge/public-api.txt`:
  - only the new annotations change, and the set of listed items is unchanged. Expect a large diff, since nearly every struct, union, and variant line gains a kind suffix.
  - non-exhaustive lines sort into one block at the top of the file
  - `VfsError` and every other non-exhaustive surface type and variant has the prefix, and `Op` and `Verdict` do not
  - `AllowAll` renders as a unit struct, and `ExecId` renders with private fields
- Exit gates, run on the finished tree before committing:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`
  - `mdbook build guide`
  - `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
- Exit-gate failures are repaired in this step's commit, like any other verification failure, even when an earlier step caused them.
- Commit: `render.rs`, the `api.rs` module docs, the `listing-tests.rs` fixtures, and the re-blessed `public-api.txt`.

</step-4>

</execution-plan>
