---
name: promptforge debt removal
overview: Remove six accepted debts added across the 30 commits in upstream/master..HEAD on promptforge - the build-xtask crate enumeration, the divergent error-chain renderers, the lost Event exhaustiveness check, the walled admin tier's inbound dependencies, and the opaque error-source newtypes - and enforce the walled-tier boundary with a checked rule.
todos:
  - id: buildxtask-enumeration
    content: Unify the build-xtask crate enumeration on one recursive walk and make the marker check report unreadable or nameless manifests instead of skipping
    status: pending
  - id: error-chain-guards
    content: Add the dedup predicate and empty-text guard to both gateway error_chain renderers with a regression test each
    status: pending
  - id: event-exhaustiveness
    content: Add the every-Event-variant and every-TaskOrigin drive tests and correct the docs that assert an unconditional guarantee
    status: pending
  - id: walled-tier-inversion
    content: Split admin/walled/handoff.rs at its cfg seam, rehome the five non-route items to homes named for what they are, and repoint the import sites
    status: pending
  - id: walled-tier-ratchet
    content: Add a build-xtask rule forbidding admin::walled paths outside the tier, allowlisting the three assembly files and skipping comments
    status: pending
  - id: error-source-gateway
    content: Create the shared error-source crate with accessors and collapse the gateway family's newtypes onto it
    status: pending
  - id: error-source-harness-workshop
    content: Collapse the harness and workshop newtypes onto the shared types, record the new component, and run the wide gates
    status: pending
isProject: false
---

# Debt removal: upstream/master..HEAD on promptforge

<product-contract>

## Product Requirements

Six debts were added or worsened by the 30 commits in `upstream/master..HEAD` on `c:\Users\Vinnie\cursor\promptforge` and are still present at `HEAD` (`0997cd78`). Three concern architecture checks and error rendering, two concern lost compile-time and enumeration guarantees, and one concerns a trust-tier boundary that nothing enforces. This work removes all six and adds the one check that keeps the tier boundary from drifting again. No wire format, HTTP status, error code, or route path changes.

- Problem and users: the maintainers of this workspace. Four architecture checks in `crates/build-xtask` silently under-report; one error concept renders three different ways across `crates/harness/runner`, `crates/gateway/app`, and `crates/gateway/cloud-providers`; a compile-time exhaustiveness check over `Event` became a conditional runtime panic; fourteen modules outside `crates/gateway/app/src/admin/walled/` depend on modules inside it, which makes the trust-tier directory an unreliable signal; and nine error newtypes across seven crates hide the third-party causes they wrap.
- Goals:
  - Every architecture check in `crates/build-xtask` enumerates the crates it claims to enumerate, and reports the crates whose manifests it cannot read.
  - One error-chain rendering contract, so a cause whose text the outer message already carries prints once at every exit.
  - The `Event` recorder's documentation states the guarantee the code actually provides, and a test makes that guarantee hold for every variant that exists.
  - No module outside `admin/walled/` depends on a module inside it, except the state and router assembly sites, and a check enforces this.
  - A caller can reach the third-party cause behind a wrapped error, and one concept is spelled one way.
- Non-goals:
  - Exposed pre-existing debt, reported but not remediated: the fourth chain renderer at `crates/gateway/app/tests/it/cuda.rs:119`; the inline test blocks in the twelve non-route gateway modules; `crates/gateway/app/src/auth.rs` carrying both test conventions at once; and `/admin/status` keeping a `serde_json::json!` body with two independent hand-written parsers.
  - The 88 `.rs` files under `crates/` exceeding 500 lines outside participating crates. The ceiling rule does not bind them.
  - Two documentation rows known to be wrong and left alone: the `--progress-width` row at `crates/workshop/server/README.md:118` says 96px where `crates/shared-ui/tokens.css:227` says 144px, and two adjacent rows in the same table were already wrong before this work; and `registry::all()` at `crates/gateway/app/src/registry.rs:65-66` claims to enumerate every mounted route while omitting five nested asset paths and two speech routes.
- Success criteria:
  - `cargo test --workspace --all-features` green.
  - `cargo clippy --workspace --all-targets --all-features` clean.
  - `cargo check -p gateway --no-default-features` green. This build is where the `handoff.rs` split changes what compiles, so it is a load-bearing gate rather than an incidental one.
  - `cargo test -p build-xtask` green, including the new enumeration and boundary tests.
  - A crate-wide search for `admin::walled::` outside `crates/gateway/app/src/admin/walled/` returns hits only in `crates/gateway/app/src/lib.rs`, `crates/gateway/app/src/registry.rs`, and `crates/gateway/app/src/test_support.rs`, and the new check fails when it returns any other file.
- Constraints:
  - No change to any wire format, persisted format, HTTP status, error code, or route path. The 35 mounted route paths and their tier assignments stay exactly as they are.
  - Changes to `crates/gateway/app` must keep the existing registry sweeps in `registry-tests.rs`, the wall tests in `loopback-tests.rs`, and the `auth.rs` test siblings passing unchanged. That those suites pass untouched is what proves the module moves are pure relocation.
  - No repository code was executed while these debts were established, so every claim in this plan is read from the diffs and the tree. Three conclusions would be firmer after a build: the headless zero-route state of `admin::walled::handoff`, the `thiserror` transparent-source delegation, and Cargo's resolution of an unregistered crate under a family container.
- Open questions: None.

## Functional Specification

Nothing user-facing changes. The observable surfaces that move are the architecture checker's violation output, the text of a rendered error cause chain at two gateway exits, and the reachability of a wrapped third-party error through a public accessor. Everything else is internal relocation held in place by the existing test suites.

- Actors and workflows:
  - A maintainer runs the architecture checker. It walks every crate under `crates/`, including those nested two levels deep, and reports any crate whose manifest it cannot read or whose package name it cannot resolve, instead of skipping it in silence.
  - A maintainer adds a module under `crates/gateway/app` that imports from the walled admin tier. The checker fails with a violation naming the file, rather than the import passing unnoticed until an unrelated commit breaks it.
  - A caller handling a wrapped error walks the cause chain or calls the accessor on the source newtype, and reaches the underlying `serde_json`, `reqwest`, or `turso` error so it can branch on it.
  - A client receives an error whose outer message already contains its source's text. It sees that text once, at a gateway exit as well as at a harness exit.
- Inputs and outputs: the checker's input is the `crates/` tree and each crate's `Cargo.toml`; its output is a violation list. The rendered cause chain's input is an error value; its output is one string per exit.
- States and validation: a crate directory is either readable with a resolvable package name, or it is a reported violation. There is no third state in which it is silently absent from the checks.
- Errors and recovery: an unreadable manifest, an unparseable manifest, and a manifest with no `[package] name` each produce a distinct violation message, matching the three forms `crates/build-xtask/src/product.rs::read_crate` already emits.
- Security and privacy behavior: the trust-tier boundary that `crates/gateway/app/src/lib.rs:57-65` and `admin/walled.rs:1-14` state in prose becomes a checked rule. Enforcement of the wall itself does not move: it stays at the router merge site in `lib.rs:485-488` and remains indifferent to which module a helper lives in. No authentication or authorization behavior changes, and no auth bypass exists today.
- Acceptance criteria:
  - The checker reports a family-named crate whose `Cargo.toml` has no `[package]` table and which carries no marker.
  - The checker returns a crate written at `crates/<container>/<subsystem>/<crate>/`.
  - The two gateway renderers and the harness renderer all print a contained cause once.
  - Every `Event` variant reaches exactly one recorder group and none reaches the catch-all arm.
  - The nine error-source newtypes are three, each with an accessor, and the cause chain reaches the third-party error.

</product-contract>
<implementation-contract>

## Technical Design

The work divides into five independent areas, one of which adds a crate to the shared substrate. These are areas of the codebase, not units of delivery: the whole of this work is one deliverable and is decomposed as a single component. The `build-xtask` changes are internal to one crate. The error-chain change is two small edits in two crates with no new dependency edge. The `Event` change is tests and documentation. The walled-tier change is intra-crate module relocation, all `pub(crate)`, plus a new checker rule. Only the error-source unification adds a component and changes public interfaces.

- Architecture:
  - One recursive crate enumeration, owned by `crates/build-xtask/src/product.rs`, replaces the two divergent walks. `tidy.rs::workspace_crates` reads `crates/` and descends exactly one level with no recursion, under a doc comment claiming "Every crate directory under `crates/`"; `product.rs::walk_crates` recurses. Of 51 manifests under `crates/`, four sit at depth 3 and only the recursive walk reaches them: `crates/gateway/stt/{api,backend-whisper,engine,whisper-ffi}`. Recursion is safe to adopt: those four are named `gateway-stt`, `gateway-stt-backend-whisper`, `gateway-stt-engine`, and `gateway-whisper-ffi`, none matches the `workshop-` or `harness-` family prefixes, and none carries the `//! ## Invariants` marker, so no new ceiling or lint binding appears and the seven files over 500 lines inside them stay exempt.
  - A new shared-substrate crate holds the unified error-source types. No existing `shared-*` crate is a home for them: `shared-vfs` owns paths and backends, `shared-loopback` owns discovery, and `shared-ui` owns styles. The new crate depends on nothing in the workspace, which is what lets `harness-log`, `workshop-workspace`, `workshop-user-state`, `gateway-api-discovery`, `gateway-local`, and `gateway-cloud-providers` all use it without creating a cross-family edge. It is a new component and belongs in the architecture record's component list.
  - No shared error renderer and no new crate for rendering. `crates/gateway/app` does not depend on `gateway-cloud-providers`, so the two byte-identical `"; "` renderers cannot delegate to each other without a new edge; each is patched in place instead.
  - The walled-tier changes are entirely inside `crates/gateway/app`. No crate boundary, dependency edge, or component ownership moves.
- Modules and interfaces:
  - `crates/build-xtask/src/tidy.rs`: `workspace_crates` is deleted; `marker_violations`, `file_ceiling_violations`, and `lint_inheritance_violations` consume the shared walk. `marker_violations` currently runs `let Some(name) = package_name(&dir) else { continue; };` where the `continue` emits nothing; it instead pushes a violation naming the directory and the failure mode. `package_name` collapses three distinct failures into one `None` through `?` on `ok()` and must distinguish them, matching the messages `read_crate` already produces: unreadable manifest, unparseable manifest, and manifest with no package name.
  - `crates/build-xtask/src/tidy.rs` gains a boundary rule: no file outside `crates/gateway/app/src/admin/walled/` may name a `crate::admin::walled::` path, with an allowlist of three files keyed on path, never on line numbers. The three are `crates/gateway/app/src/lib.rs` for the `AppState` field types and the router merge, `crates/gateway/app/src/registry.rs` for the route enumeration, and `crates/gateway/app/src/test_support.rs` for the fixture that assembles the same state. These three are the state and fixture assembly sites, which necessarily name the tier's modules; every other outside reference is a module in the wrong place and is moved rather than allowlisted.
  - The rule is a textual match and its reach is limited: it catches a spelled `crate::admin::walled::` path and misses an alias, a `super::` path, a re-export, and any path a macro generates. It is a tripwire for the common case, not a proof of the boundary. It must also skip comment lines, since module docs legitimately name a module path in prose, as `commands-apply.rs` does when it points at the route side of the apply command.
  - `crates/gateway/app/src/error.rs::error_chain` and `crates/gateway/cloud-providers/src/lib.rs::error_chain` gain the dedup predicate and empty-text guard that `crates/harness/runner/src/display_chain.rs::display_chain` already has. The `"; "` separator stays in both; the separator difference between harness and gateway exits is established and deliberate.
  - `crates/gateway/app/src/admin/walled/handoff.rs` splits along the seam its own `cfg` attributes already draw. The ungated auth primitives - `AUTH_COOKIE`, `presented_cookie_proof`, `session_token`, `fetch_metadata_allows_cookie`, `fetch_metadata_allows_ambient`, `hex_decode`, `hex_digit`, and `auth_url` - move to a neutral crate-root module beside `auth.rs`. The `config-ui`-gated route material - `CONFIG_REDIRECT`, `AUTH`, `ROUTES`, `routes()`, `auth_handoff`, `config_ui_redirect`, `AuthQuery`, and `hex_encode` - stays walled. The module declaration at `admin/walled.rs:23` then carries the same `cfg` as its merge at lines 61-62, which removes the anomaly where a headless build leaves a routeless module inside a directory defined as "routes that read secrets in plaintext, write files, or launch processes".
  - Five things move out of the tier, each to a home named for what it is rather than to one catch-all module. `config_write_error` goes from `admin/walled/config.rs` into `error.rs`, following the precedent already set when `error_chain` moved there, because it is an error. The other four are not errors and `error.rs` is the wrong home for them: `SpeechSnapshot` goes from `admin/walled/system.rs` into `speech.rs`, which already owns the speech surface it reports on; `ShutdownSignal` goes from `admin/walled/shutdown.rs` into a new crate-root `shutdown.rs`; the three `cloud_models` boot constants `SHEET_URL_ENV`, `DEFAULT_SHEET_URL`, and `CACHE_FILE_NAME` go into `boot.rs`, which is documented as boot-time configuration and is what `runner.rs` reads them for; and the `config_pending` path helpers `shadow_census`, `config_root`, `canonical_form`, and `relative_name`, together with the `ShadowCensus` type `shadow_census` returns, go into a new crate-root `config_shadow.rs`. The tier's own route modules keep using all five through inward references, which the boundary rule does not restrict.
  - `crates/promptforge-api-runtime/src/test_support/recording-forward.rs`: the doc comment on `forward_one` asserts "the recorder exists to observe every event", which the code no longer guarantees; it is rewritten to state the conditional guarantee and name the test that backs it. The inline comment at `crates/promptforge-api-runtime/src/execute/scheduler/tasks.rs:406-414` is corrected the same way.
- File and public API changes:
  - New crate under `crates/` in the shared substrate, exporting three types that wrap `serde_json::Error`, `reqwest::Error`, and `turso::Error`. Each is `#[derive(Debug, thiserror::Error)] #[error(transparent)]` with a private field, an `into_inner` and an `as_inner` accessor, and a `From` conversion. The three third-party dependencies are optional and each sits behind its own feature, so `gateway-api-discovery` does not acquire `turso` and `harness-log` does not acquire `reqwest`.
  - The nine existing newtypes are removed and their uses repointed: `JsonSource` at `crates/gateway-api-discovery/src/error.rs:13`, `crates/gateway/local/src/error.rs:12`, `crates/workshop/user-state/src/error.rs:70`, and `crates/workshop/workspace/src/error.rs:190`; `HttpSource` at `crates/gateway/local/src/error.rs:25` and `crates/gateway/cloud-providers/src/lib.rs:133`; `DatabaseSource` at `crates/harness/log/src/error.rs:51,58` and `crates/workshop/workspace/src/workspace_file.rs:122`. `PayloadSource` wraps `serde_json::Error` and folds into the shared JSON type. This is a breaking change to the public variant shapes of `LogError`, `WorkspaceFileError`, `WorkspaceError`, `UserStateError`, `DiscoveryError`, `LocalError`, and `FetchError`, all within this workspace.
  - `vibe/archdoc.md`'s component list gains the new shared-substrate crate.
- Data, persistence, failure, security, and privacy constraints:
  - `#[error(transparent)]` delegates both `Display` and `source()` to the inner value, which is why the wrapped error is currently unreachable through the chain; the accessor is what restores branching. Five sites already depend on that reachability for unwrapped errors, at `crates/harness/models/src/catalog.rs:327,431`, `crates/harness/models/src/transport/tests/limits.rs:193`, `crates/promptforge/lua/src/protocol/tests/answer.rs:265`, and `crates/promptforge/model-client/src/client/stream-tests.rs:326`.
  - Adding dedup to the gateway renderers changes rendered strings at gateway exits. No test pins those strings today, which is both why the change is cheap and why it lands with its own regression tests.
  - The tier boundary check must not weaken the wall. Enforcement stays structural at the router merge; the check protects the directory's meaning, not the wall itself.

</implementation-contract>
<verification-contract>

## Testing Plan

Each area carries its own focused tests, and the walled-tier relocation is verified by existing suites passing unchanged rather than by new tests. The wide gates run once at the end.

- Unit:
  - A `build-xtask` test writing a directory named as a harness crate whose `Cargo.toml` has no `[package]` table and which carries no marker, asserting the marker check names it and that it is included in the participating set. Shape it on the existing `a_harness_crate_without_the_marker_is_a_violation_and_still_held_to_the_ceiling` fixture and keep it beside `a_tiered_crate_whose_manifest_is_missing_is_reported_not_skipped`, whose principle it restores.
  - A `build-xtask` test asserting the walk returns a crate written at `crates/<container>/<subsystem>/<crate>/`.
  - A `build-xtask` test asserting the boundary rule fires for a file outside the walled tier naming a `crate::admin::walled::` path, and stays silent for each allowlisted assembly site.
  - One regression test per gateway renderer: an error whose outer message already contains its source's text renders that text once. Mirror the assertion the harness renderer is already covered by, so all three are pinned to the same contract.
  - A test constructing one value of every `Event` variant and driving each through `forward_one`, asserting each lands in exactly one group and none reaches the catch-all arm.
  - A test driving every `TaskOrigin` variant through `settle_all_tasks`, asserting only `Author` reaches the leaked list.
  - A test on the shared error-source types asserting the cause chain reaches the wrapped third-party error and that the accessor returns it.
- Integration and end-to-end:
  - A workspace-level test asserting the crate count found by the tidy checks equals the count found by the product checks. This is the regression that prevents the two enumerations diverging again.
- Regression, security, and performance:
  - The registry sweeps in `registry-tests.rs`, the wall tests in `loopback-tests.rs`, and the `auth.rs` test siblings must pass unchanged after the walled-tier moves. Any edit to those suites is a signal the relocation was not pure.
  - `cargo check -p gateway --no-default-features` after the `handoff.rs` split, because that configuration is gated but not test-run and is the one the split changes.
- Exit criteria:
  - `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features`, `cargo check -p gateway --no-default-features`, and `cargo test -p build-xtask` all green.
  - A crate-wide search for `admin::walled::` outside the tier returns only the three assembly sites, `lib.rs`, `registry.rs`, and `test_support.rs`, and the new boundary rule enforces that result.

</verification-contract>
<decision-record>

## Decision Record

Six remediation choices are settled. Three were escalated because they change a public interface, component ownership, or add a structural check, and the user resolved all three.

- Decisions:
  - Collapse the nine error-source newtypes onto three shared types AND give each an accessor. The user chose both together over either alone: the accessor restores cause branching, the unification removes four concepts copied nine ways. Consequence: a breaking change to seven crates' public variant shapes, and a new shared-substrate component. The three third-party dependencies are feature-gated so no crate acquires a dependency it does not use.
  - Name the new shared-substrate crate `shared-error-source`, at `crates/shared-error-source/`, exporting `JsonSource`, `HttpSource`, and `DatabaseSource` behind the features `json`, `http`, and `database`. Chosen to match the `shared-*` prefix every substrate crate already carries and to keep the three surviving names identical to the newtype names they replace, so the migration is a path change rather than a rename. Consequence: the name lands in six crates' manifests, seven error enums, and `vibe/archdoc.md`, so it is expensive to change once step 8 is committed.
  - Fix the `Event` exhaustiveness loss with a test and corrected documentation only. The user chose this over changing the event macro, accepting that the guarantee stays conditional on someone updating the test when a variant is added. Record it as a partial fix rather than a resolution.
  - Add the checked rule forbidding `admin::walled::` paths outside the tier. The user approved the exception. It qualifies: the tier-as-directory statement is a trust-boundary contract, nothing else protects it because the wall is applied at the merge site and is indifferent to module location, and the invariant has already broken once silently. The checker harness already exists and is being modified in this work anyway.
  - Patch the two gateway error renderers in place rather than unifying them. Rationale: `crates/gateway/app` does not depend on `gateway-cloud-providers`, so unification needs a new shared home and crosses two components with no edge between them. Tradeoff: the `"; "` and `": "` separators stay divergent, so an error still renders differently at a harness exit than at a gateway exit.
  - Delete the tidy crate walk and share the product walk, rather than adding recursion to both. Tradeoff: the tidy checks take a dependency on the product enumeration's shape, which is the point, since one enumeration cannot diverge from itself.
  - Do the walled-tier work as intra-crate module relocation. All moved items are `pub(crate)` and no public, wire, persisted, or cross-component surface is involved.
  - Send each item leaving the tier to a home named for what it is, and use `error.rs` only for the one item that is an error. `SpeechSnapshot` goes to `speech.rs`, `ShutdownSignal` to a new `shutdown.rs`, the boot constants to `boot.rs`, and the path helpers to a new `config_shadow.rs`. Rationale: following the `error_chain` precedent literally would put a state snapshot, a shutdown signal, boot constants, and path helpers in a module named for errors, which is the same grab-bag smell this work is removing elsewhere.
  - Sequence the work as seven steps and give every step the same single component name, so no step but the last sits at a component boundary. This is deliberate and governs: the areas named in the technical design are regions of the codebase, not units of delivery, and splitting them into separate components would force a wide verification pass at internal boundaries where a focused one suffices. Carry no separate step for verification. The walled-tier split and the rehoming that goes with it are one relocation with one exit condition, so they are one step. The wide gates belong to the last step rather than to a step of their own, since a step that only runs gates produces a commit with no code and the final step's verification already runs them.
  - Split the error-source collapse across two steps along the family line rather than landing it as one commit. Creating the crate, deleting nine newtypes, repointing seven enums across six crates, touching the architecture record, and running every wide gate in a single commit is hard to bisect when a gate goes red. Step 6 creates the crate and repoints the three gateway consumers, which keeps it from shipping dormant; step 7 repoints the harness and workshop consumers and runs the gates. Two bisect points instead of one, at the cost of one extra step.
- Rejected alternatives:
  - Accessors alone on the nine newtypes: cheapest and non-breaking, but leaves four concepts copied nine ways. Revisit if the unification proves more disruptive than expected.
  - Removing `#[non_exhaustive]` from `Event`: the simplest route to an unconditional compile check, rejected because it reverses a deliberate downstream-stability decision. Revisit if the conditional test guarantee is observed to fail in practice.
  - Teaching the event macro to emit the recorder's group patterns: restores an unconditional check but moves a runtime concept into the types crate. Revisit with the next change to the event vocabulary.
  - Re-exporting `auth_url` from a neutral path instead of splitting `handoff.rs`: cheaper, but leaves the tier signal unreliable and the routeless-module anomaly in place.
  - Leaving the tier boundary to convention and review: rejected because review already failed to catch one violation.
- Assumptions, risks, and notes:
  - The walled tier currently has fourteen dependents outside it, at `auth.rs:23`, `admin/open/status.rs:14`, `admin/open/progress.rs:15`, `admin/open/progress-tests.rs:12`, `admin/open/profiles.rs:13`, `commands-apply.rs:23,24`, `lib.rs:132`, `runner.rs:858,1016,1019,1030,1128`, `relaunch.rs:66,76`, `tray/{linux,macos,windows}.rs`, and `test_support.rs:58,77,91`. The sharpest is `check_auth` at `auth.rs:183-210`, the crate's single authentication predicate, which calls four functions from the walled `handoff` module and serves the open tier, the walled tier, and the speech surface through three callers.
  - The state and fixture assembly sites are accepted and stay, and they are exactly three files: `lib.rs` holds walled module types as `AppState` fields and merges the walled routes, `registry.rs` enumerates them, and `test_support.rs` builds the fixture that assembles the same state, naming `hf::HfProxy`. These are where the tier's modules are legitimately named, and they are the boundary rule's whole allowlist.
  - Everything else reaching into the tier is a module in the wrong place and moves out, including three that an earlier draft of this plan missed: the `cloud_models` boot constants that `runner.rs` reads, the `config_pending` path helpers that the apply command body uses, and the `config_apply` path spelled in a `commands-apply.rs` module doc. Missing them would have made the boundary rule fire on `commands-apply.rs`, `runner.rs`, and `test_support.rs` the moment it landed, and the exit check unsatisfiable.
  - `HfProxy` stays in the `hf` route module rather than moving out to satisfy the rule. It is the proxy that route owns and that `AppState` holds; relocating it would let the checker dictate the design. Allowlisting the fixture that assembles it is the honest answer.
  - Three `tray/` files were declared off-limits by an earlier plan's scope. That scope does not bind this work; the change there is three import lines.
  - Risk: the error-source unification is the largest single change and the only one touching public interfaces across three families. It is independent of every other area and can be sequenced last.
  - Risk: the boundary rule's allowlist is the fiddly part. Too narrow and legitimate assembly fails; too wide and the rule stops catching anything. The three named files are the whole allowlist.
  - Risk: the boundary rule is a textual match on a spelled `crate::admin::walled::` path, so it misses an alias, a `super::` path, a re-export, and any path a macro generates. It catches the shape every current violation takes and would have caught the one that already slipped through, but it is a tripwire rather than a proof, and the plan should not be read as claiming the boundary is now airtight. It also reads source text with no view of `cfg`, which is why a `#[cfg(test)]` file like `test_support.rs` still has to be allowlisted by name.

### Deferred and Out of Scope

- Deferred: the fourth chain renderer at `crates/gateway/app/tests/it/cuda.rs:119`, which joins with `": "` rather than the gateway's `"; "`. Revisit when the gateway test helpers are next touched.
- Deferred: the `--progress-width` documentation row and the two adjacent rows already wrong before this work, at `crates/workshop/server/README.md:118`. Revisit by asserting the table against `crates/shared-ui/tokens.css` rather than by hand-correcting rows.
- Deferred: the `registry::all()` doc comment at `crates/gateway/app/src/registry.rs:65-66`, which claims to enumerate every mounted route while omitting five nested asset paths and two speech routes, enumerating 35 of 42. Revisit when the registry next changes.
- Out of scope: the inline test blocks in the twelve non-route gateway modules, and `crates/gateway/app/src/auth.rs` carrying both test conventions at once.
- Out of scope: `/admin/status` keeping a `serde_json::json!` body with two independent hand-written parsers, in the configuration interface and at `crates/workshop/gateway/src/heartbeat-refresh.rs:48-54`. Both parsers predate this work.
- Out of scope: the 88 `.rs` files over 500 lines outside participating crates.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (plain `cargo build` resolves to the same default member, `crates/gateway/app`); the desktop app is the explicit `cargo build --locked -p workshop`; the two TypeScript bundles build with `npm ci && npm run build` in `crates/workshop/ui` and `crates/gateway/config-ui/ui`, and the cargo build scripts drive the same bundles once `npm ci` has run.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test_name_substring>`; for a specific integration case the repository also uses `cargo test --locked -p <crate> --test it <test_name>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for workspace crates, `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the workshop partition (plus `cargo nextest run --locked -p workshop-server --features headless`), and `npm test` inside a UI package directory for the TypeScript side.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the workshop partition `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`, plus the boundary harness `cargo test -p build-xtask`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, and for the workshop partition `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Clippy is a superset of `cargo check` and shares no artifacts with it, so no standalone `cargo check --workspace` runs beside it; the one sanctioned check is the headless shape `cargo check -p gateway --no-default-features`. TypeScript type checking is `npm run typecheck` in each UI package.
- Formatter check command: `cargo fmt --all --check`.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; the user guide builds with `mdbook build guide`.
- Test placement and naming conventions: integration tests live in one `tests/it/` binary per crate, with `tests/it/main.rs` declaring the modules, a `support.rs` beside them for helpers, and a `#![expect(clippy::expect_used, clippy::unwrap_used, reason = ...)]` header on that `main.rs`; deeper subject areas nest as `tests/it/<area>/<case>.rs` beside `tests/it/<area>.rs`. Unit tests sit inline as `#[cfg(test)] mod tests` in the module they cover, or in a `src/<module>/tests/` subdirectory when that block outgrows the file. Test function names are full sentences in snake_case describing the behavior, often opening with an article, for example `a_cancel_writes_one_dropped_answer_per_outstanding_effect`. TypeScript tests run on the node test runner and are matched as `test/**/*.mjs` and `src/**/*.test.mjs`.
- Directory map: `crates/` holds every Rust crate, with the public layer at `crates/` root (`promptforge-api-runtime`, `promptforge-api-types`, `gateway-api-types`, `gateway-api-discovery`, `harness-api`, `shared-loopback`, `shared-vfs`, the `build-*` meta crates, and `workspace-hack`) and four manifestless family containers beneath it (`crates/promptforge/`, `crates/gateway/` including the nested `crates/gateway/stt/`, `crates/workshop/`, `crates/harness/`) that are private to their families; `crates/shared-ui/` and `crates/workshop/ui/` and `crates/gateway/config-ui/ui/` are npm packages rather than Rust crates. `guide/` is the mdbook user guide, `prompts/` holds PromptForge prompt sources, `tools/` holds node-based repository tooling with `.test.mjs` siblings, `vibe/` holds the architecture document and the dated plan archive, `.github/workflows/` holds CI, `.githooks/` holds the pre-commit and pre-push hooks, `.cargo/config.toml` defines the `cargo workshop` and `cargo xtask` aliases, `local/` and `images/` hold local configuration and assets, and `target/` and `target-msrv/` are build output.
- Component boundaries: the executor (`promptforge-api-runtime` over the private `crates/promptforge/` crates) is a sans-I/O deterministic state machine depending on store, the Lua VM boundary, and shared substrate; the harness (`harness-api` over `crates/harness/`) is the executor's only production host, owning the tokio runtime, performers, capabilities, sessions, and the Turso run log, and depends on the executor, the gateway's public pair, store, and shared substrate; the gateway (`gateway-api-types` and `gateway-api-discovery` over `crates/gateway/`) is an independent server owning model routing, provider access, and local inference, depending only on shared substrate; the workshop (`crates/workshop/`, shell package `workshop`) is the Tauri desktop shell plus in-process server, depending on the harness through `harness-api`, the gateway through its public pair, store, and shared substrate, and the shell sees the server only through `workshop-server-api`; the store is a run-scoped facade over the VFS layer (`shared-vfs` plus the `promptforge-vfs` policy gate), which depends on nothing; shared substrate crates (`shared-*`) depend on no product crate. A crate inside a family container may depend only on crates at the `crates/` root and its own siblings, `build-*` crates are exempt from container privacy, and dependency direction inside the workshop SPA runs shell to features to services to vocabulary.
- Conventions summary: `cargo test -p build-xtask` enforces the tier graph, the product-boundary matrix, the one-door rules, container privacy, lint inheritance, the mandatory `## Invariants` doc marker that opens every `workshop-*` and `harness-*` crate's `lib.rs`, and a 500-line ceiling on every `.rs` file in a crate carrying that marker, with the Tauri shell exempt from the marker and the ceiling. Workspace lints forbid `unsafe_code`, warn on `missing_docs`, `missing_debug_implementations`, and `unreachable_pub`, deny clippy `all` and `pedantic` along with `unwrap_used` and `expect_used`, and deny broken and private intra-doc links; edition 2024, resolver 3, one shared version and license across members, and all third-party versions pinned once in `[workspace.dependencies]` with a comment explaining every non-obvious pin. Source directories are flat by default: a subdirectory needs at least three files, and one or two files instead live beside the parent as `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`, converting in both directions as a group crosses the line. Comments explain a non-obvious constraint, ordering requirement, or workaround, and every platform or upstream-bug workaround cites its issue URL. Behavior changes ship with tests in the same change, and a plan may not introduce a source parser, snapshot, allowlist, count, ceiling, topology check, or other structural enforcement without explicit user approval. Error and status messages are written for model consumption: concise, factual, self-contained, naming required versus actual. On the SPA side, CSS is colocated with its TypeScript in self-contained feature directories, component CSS uses only `--ws-*` tokens, classes carry the `.ws-` prefix, files and directories are kebab-case, every directory has an `index.ts` barrel, feature directories lazy-load through dynamic `import()` and never import the boot shell, and `localStorage` is banned in favor of the `ui-storage` adapter to the server.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: One crate enumeration that reports what it cannot read [completed]

- Component: debt-removal
- Excluded from this and every later step: the deferred and out-of-scope entries in the decision record, and any change to a wire format, persisted format, HTTP status, error code, or route path.
- Delete `workspace_crates` from `crates/build-xtask/src/tidy.rs` and have `marker_violations`, `file_ceiling_violations`, and `lint_inheritance_violations` consume the recursive walk in `crates/build-xtask/src/product.rs::walk_crates`.
- Replace the `?`-on-`ok()` collapsing in `package_name` with a three-way outcome distinguishing an unreadable manifest, an unparseable manifest, and a manifest with no `[package] name`, reusing the three messages `product.rs::read_crate` already emits. Reuse means calling the shared reader, not copying its message strings into `tidy.rs`: a second copy of the messages recreates the divergence this step exists to remove. Change `marker_violations`'s `let Some(name) = package_name(&dir) else { continue; };` to push a violation naming the directory and the failure mode.
- Tests in `crates/build-xtask`: the walk returns a crate written at `crates/<container>/<subsystem>/<crate>/`; a family-named directory whose `Cargo.toml` has no `[package]` table and which carries no marker is named by the marker check and still held to the ceiling, shaped on `a_harness_crate_without_the_marker_is_a_violation_and_still_held_to_the_ceiling` and placed beside `a_tiered_crate_whose_manifest_is_missing_is_reported_not_skipped`; and the crate count the tidy checks find equals the count the product checks find.
- Verify with `cargo test -p build-xtask`.

</step-1>

<step-2>

### Step 2: One error-chain rendering contract at both gateway exits [completed]

- Component: debt-removal
- Add the dedup predicate and the empty-text guard that `crates/harness/runner/src/display_chain.rs::display_chain` already carries to `crates/gateway/app/src/error.rs::error_chain` and to `crates/gateway/cloud-providers/src/lib.rs::error_chain`. Both keep the `"; "` separator; the separator difference from the harness exit is deliberate and stays.
- One regression test per crate, mirroring the assertion the harness renderer is already pinned to: an error whose outer message already contains its source's text renders that text once. No test pins these strings today, so both tests are new.
- Verify with `cargo nextest run --locked -p gateway --all-features` and `cargo nextest run --locked -p gateway-cloud-providers --all-features`.

</step-2>

<step-3>

### Step 3: Event and TaskOrigin exhaustiveness pinned by tests [completed]

- Component: debt-removal
- Add a test constructing one value of every `Event` variant and driving each through `forward_one` in `crates/promptforge-api-runtime/src/test_support/recording-forward.rs`, asserting each lands in exactly one recorder group and none reaches the catch-all arm.
- Add a test driving every `TaskOrigin` variant through `settle_all_tasks` in `crates/promptforge-api-runtime/src/execute/scheduler/tasks.rs`, asserting only `Author` reaches the leaked list.
- Rewrite the `forward_one` doc comment, which asserts "the recorder exists to observe every event", and the inline comment at `tasks.rs:406-414`, so both state the conditional guarantee and name the test that backs it. This is a partial fix by decision: the guarantee holds only while someone updates the test when a variant is added.
- Verify with `cargo nextest run --locked -p promptforge-api-runtime --all-features`.

</step-3>

<step-4>

### Step 4: Walled-tier relocation - the handoff split and the five rehomed items [completed]

- Component: debt-removal
- Move the ungated auth primitives out of `crates/gateway/app/src/admin/walled/handoff.rs` into a new `crates/gateway/app/src/auth-primitives.rs`, wired from `auth.rs` as `#[path = "auth-primitives.rs"] mod primitives;` per the flat source convention: `AUTH_COOKIE`, `presented_cookie_proof`, `session_token`, `fetch_metadata_allows_cookie`, `fetch_metadata_allows_ambient`, `hex_decode`, `hex_digit`, and `auth_url`. Everything moved stays `pub(crate)`.
- Leave the `config-ui`-gated route material walled: `CONFIG_REDIRECT`, `AUTH`, `ROUTES`, `routes()`, `auth_handoff`, `config_ui_redirect`, `AuthQuery`, and `hex_encode`. Put the same `cfg` on the module declaration at `admin/walled.rs:23` that its merge at lines 61-62 already carries, which removes the routeless-module-in-a-walled-directory anomaly in a headless build.
- Repoint every site naming a moved primitive, starting with `check_auth` at `auth.rs:183-210`, which calls four of them and serves the open tier, the walled tier, and the speech surface through three callers, and covering `runner.rs`, `relaunch.rs`, `tray/{linux,macos,windows}.rs`, and `commands-apply.rs`. The `tray/` change is three import lines; the earlier plan's off-limits scope does not bind this work.
- Move five more things out of the tier, each to a home named for what it is. `config_write_error` from `admin/walled/config.rs` into `crates/gateway/app/src/error.rs`, following the precedent set when `error_chain` moved there. `SpeechSnapshot` from `admin/walled/system.rs` into `speech.rs`, which already owns the speech surface it reports on. `ShutdownSignal` from `admin/walled/shutdown.rs` into a new crate-root `shutdown.rs`. The three boot constants `SHEET_URL_ENV`, `DEFAULT_SHEET_URL`, and `CACHE_FILE_NAME` from `admin/walled/cloud_models.rs` into `boot.rs`, whose module doc already declares it the home of boot-time configuration and which is what `runner.rs` reads them for. The path helpers `shadow_census`, `config_root`, `canonical_form`, and `relative_name`, plus the `ShadowCensus` type that `shadow_census` returns, from `admin/walled/config_pending.rs` into a new crate-root `config_shadow.rs`. Everything moved stays `pub(crate)`. Do not put the four non-error items in `error.rs`: a module named for errors housing a state snapshot, a shutdown signal, boot constants, and path helpers is the grab bag this work exists to avoid.
- The tier's own route modules keep using all five, now through inward references such as `crate::boot::CACHE_FILE_NAME`. That direction is fine and the boundary rule does not restrict it. The `cloud_models` tests inside the tier that use `CACHE_FILE_NAME` are repointed the same way.
- Repoint every site naming one of the five: `admin/open/status.rs`, `admin/open/progress.rs`, `admin/open/progress-tests.rs`, `admin/open/profiles.rs`, `commands-apply.rs`, `runner.rs`, and the `AppState` fields in `lib.rs`, leaving in place whatever walled types `lib.rs` still legitimately holds.
- Reword the `commands-apply.rs` module doc that spells `admin::walled::config_apply` in prose so it names the route side without writing the path, since step 5's rule scans text. Step 5 also skips comment lines, so this is belt and braces rather than the only defense.
- After this step, the only files outside `crates/gateway/app/src/admin/walled/` naming a `crate::admin::walled::` path are three: `lib.rs` for the `AppState` field types and the router merge, `registry.rs` for the route enumeration, and `test_support.rs` for the fixture that assembles the same state, which names `hf::HfProxy`. `HfProxy` deliberately stays in its own route module; moving a route's proxy type out of that route to satisfy a checker would be the rule dictating the design. Confirm the three by crate-wide search before committing, because step 5's allowlist is exactly those files. Identify sites by file and role, never by line number, since this step moves the lines.
- This step is pure relocation, so its verification is the existing suites passing unedited: the registry sweeps in `registry-tests.rs`, the wall tests in `loopback-tests.rs`, and the `auth.rs` test siblings. Any edit to those suites is a signal the relocation was not pure. Also run `cargo nextest run --locked -p gateway --all-features` and `cargo check -p gateway --no-default-features`, the gated build the `handoff.rs` split changes and the load-bearing gate for this step.

</step-4>

<step-5>

### Step 5: Walled-tier boundary rule in the checker [completed]

- Component: debt-removal
- Add a rule to `crates/build-xtask/src/tidy.rs`: no file outside `crates/gateway/app/src/admin/walled/` may name a `crate::admin::walled::` path. The allowlist is exactly three files, `crates/gateway/app/src/lib.rs` for the `AppState` field types and the router merge, `crates/gateway/app/src/registry.rs` for the route enumeration, and `crates/gateway/app/src/test_support.rs` for the fixture that assembles the same state. Key the allowlist on file paths, never on line numbers, which move with any edit. Too narrow and legitimate assembly fails; too wide and the rule catches nothing.
- Skip comment lines. A module doc that names a route module in prose is not a dependency, and treating it as one would fail the build on documentation. `test_support.rs` is `#[cfg(test)]`-gated but must still be allowlisted by name: the rule reads source text and never sees a cfg, so "it is test-only" is not an exclusion the rule can make.
- State the rule's reach in its own doc comment. A textual match catches a spelled `crate::admin::walled::` path and misses an alias, a `super::` path, a re-export, and any path a macro generates. It is a tripwire for the common case, not a proof of the boundary, and it should not be described as one.
- The rule protects the directory's meaning, not the wall. Wall enforcement stays structural at the router merge in `lib.rs` and does not move.
- Tests in `crates/build-xtask`: the rule fires, naming the file, for a non-allowlisted file outside the tier that names such a path; stays silent for each of the three allowlisted files; and stays silent for a comment line that spells the path.
- Must land after step 4, or it fails on the very imports that step removes, and after step 1, which reshapes the same file. Verify with `cargo test -p build-xtask` and by running the checker against the tree as step 4 left it.

</step-5>

<step-6>

### Step 6: shared-error-source crate and the gateway newtypes collapsed onto it [completed]

- Component: debt-removal
- Create `crates/shared-error-source/` (package `shared-error-source`), registered in the workspace members, depending on no workspace crate. That independence is what lets `harness-log`, `workshop-workspace`, `workshop-user-state`, `gateway-api-discovery`, `gateway-local`, and `gateway-cloud-providers` all use it without a cross-family edge.
- Export `JsonSource`, `HttpSource`, and `DatabaseSource`, wrapping `serde_json::Error`, `reqwest::Error`, and `turso::Error`. Each is `#[derive(Debug, thiserror::Error)]` with `#[error(transparent)]`, a private field, `into_inner` and `as_inner` accessors, and a `From` conversion.
- Gate the three third-party dependencies as optional behind the features `json`, `http`, and `database`, so `gateway-api-discovery` does not acquire `turso` and `harness-log` does not acquire `reqwest`.
- Tests, one per type: the accessor returns the wrapped third-party error, and the cause chain reaches it. `#[error(transparent)]` delegates both `Display` and `source()`, which is why the accessor is the part that restores branching.
- Collapse the gateway family's newtypes in this same commit, so the crate ships with real consumers rather than dormant: remove `JsonSource` at `crates/gateway-api-discovery/src/error.rs` and `crates/gateway/local/src/error.rs`, and `HttpSource` at `crates/gateway/local/src/error.rs` and `crates/gateway/cloud-providers/src/lib.rs`. Repoint `DiscoveryError`, `LocalError`, and `FetchError`, adding `shared-error-source` with only the needed feature to each of the three crates. This breaks those public variant shapes, all within this workspace.
- Tests: for each of the three repointed enums, a case asserting the wrapped third-party error is reachable through the shared type, either by accessor or by walking the chain.
- Verify with `cargo nextest run --locked -p shared-error-source --all-features`, `cargo nextest run --locked -p gateway-api-discovery -p gateway-local -p gateway-cloud-providers --all-features`, and `cargo test -p build-xtask`, which checks the tier graph, container privacy, and lint inheritance for the new crate.

</step-6>

<step-7>

### Step 7: harness and workshop newtypes collapsed, and the wide gates

- Component: debt-removal
- Remove `DatabaseSource` at `crates/harness/log/src/error.rs` and `crates/workshop/workspace/src/workspace_file.rs`, and `JsonSource` at `crates/workshop/user-state/src/error.rs` and `crates/workshop/workspace/src/error.rs`. Fold `PayloadSource`, which wraps `serde_json::Error`, onto the shared JSON type. Repoint `LogError`, `WorkspaceFileError`, `WorkspaceError`, and `UserStateError`, adding `shared-error-source` with only the needed feature to each of the three crates.
- Splitting the collapse across steps 6 and 7 along the family line is deliberate, so a red gate has two bisect points instead of one: the gateway repoint and the harness and workshop repoint fail independently.
- Keep the five sites that already depend on chain reachability passing: `crates/harness/models/src/catalog.rs:327,431`, `crates/harness/models/src/transport/tests/limits.rs:193`, `crates/promptforge/lua/src/protocol/tests/answer.rs:265`, and `crates/promptforge/model-client/src/client/stream-tests.rs:326`.
- Tests: for each of the four repointed enums, a case asserting the wrapped third-party error is reachable through the shared type, either by accessor or by walking the chain.
- Add `shared-error-source` to the component list in `vibe/archdoc.md` under shared substrate.
- Verify this step's own change with `cargo nextest run --locked -p harness-log --all-features` and the workshop partition `cargo nextest run --locked -p workshop-workspace -p workshop-user-state`.
- Then run the wide gates once over the finished tree. They belong to this step rather than a step of their own, by decision, because a step that only runs gates produces a commit with no code: the full suite `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, the doctests, the workshop partition and its doctests, `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and the workshop partition's clippy, `cargo fmt --all --check`, `cargo check -p gateway --no-default-features`, and the docs build with `RUSTDOCFLAGS="-D warnings"`.
- Confirm a crate-wide search for `admin::walled::` outside `crates/gateway/app/src/admin/walled/` returns hits only in `crates/gateway/app/src/lib.rs`, `crates/gateway/app/src/registry.rs`, and `crates/gateway/app/src/test_support.rs`, and that the step 5 rule fails when it returns any other file.
- Fix fallout in place. Any fix that changes behavior carries its test in the same commit.

</step-7>

</execution-plan>
