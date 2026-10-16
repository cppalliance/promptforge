---
name: Remove hakari
overview: Remove the workspace-hack crate and cargo-hakari from runvil, delete workshop-server's `headless` feature, and fold the five Windows Workshop test runs into one nextest command, while keeping what the shipped gateway and Workshop do at runtime unchanged.
todos:
  - id: baseline
    content: Capture baseline cargo tree feature lists for gateway and workshop on the five targets (scratch)
    status: completed
  - id: headless
    content: Delete workshop-server's headless feature, NoopAssets, the one-impl AssetServer trait, and the headless tests
    status: completed
  - id: hakari-out
    content: Remove workspace-hack crate, hakari.toml, 49 manifest lines, root workspace dep, CI hakari steps, hakari comments; refresh Cargo.lock
    status: completed
  - id: xtask-docs
    content: Drop workspace-hack allowances from build-xtask product and tidy-wiring rules and tests; update Invariants docs and guard comments
    status: completed
  - id: sweep
    content: Per-package cargo check sweep (default and all-targets/all-features); add missing features crate by crate; commit with hakari removal
    status: completed
  - id: feature-diff
    content: Diff shipped closures against baseline; restore runtime features (reqwest system-proxy); confirm ring and Engine tokio checks
    status: completed
  - id: ci-merge
    content: Replace the four Windows test steps with one nextest command; delete the Linux duplicate race-test step
    status: completed
  - id: verify
    content: Run the Testing Plan exit criteria and the merged Windows command locally; CI green; dispatch release-workshop.yml on the branch
    status: completed
isProject: false
---

# Remove hakari and cut the Workshop test runs to one

<product-contract>

## Product Requirements

The runvil repository uses cargo-hakari's `workspace-hack` crate to give every workspace crate one shared third-party feature set. That compiles test-only features into the shipped gateway and Workshop binaries, puts the async and HTTP stack under the sans-I/O Engine crates, and carries Workshop-only features into the gateway. This change removes hakari entirely, deletes the `headless` feature of `workshop-server`, and folds the Windows Workshop CI job's five test commands into one. Shipped runtime behavior stays the same, apart from the intended removals.

- Problem and users:
  - Users: the repository owner, the agents that build, test, and change runvil, and CI. All paths below are relative to the runvil repository root.
  - `crates/workspace-hack/Cargo.toml:69` and `:142` list tokio with `test-util` as a normal and a build dependency. Every direct request for `test-util` is a dev-dependency: `crates/runvil-internal/engine/Cargo.toml:44`, `crates/gateway/app/Cargo.toml:158`, `crates/workshop/server/Cargo.toml:83`, `crates/workshop/support/Cargo.toml:28`, `crates/workshop/status/Cargo.toml:17`, and `crates/gateway/stt/engine/Cargo.toml:16`. Release builds compile it anyway.
  - Six of the seven Engine crates depend on `workspace-hack`: `crates/runvil/Cargo.toml:18`, plus the `workspace-hack.workspace = true` lines in `crates/runvil-internal/{engine,lua,model-client,parser,types}/Cargo.toml`. The seventh, `crates/runvil-internal/vfs`, declares no dependencies at all. So `cargo build -p runvil-types` compiles tokio, reqwest, hyper, and Workshop GUI crates.
  - The hack has already forced one workaround. `tauri-plugin-updater`'s default `rustls-tls` feature would have put `ring` into every member, the gateway included (`Cargo.toml:242-251`).
  - The Windows `check-workshop` job runs five test commands on different package sets (`.github/workflows/ci.yml:182-210`). The Linux `check-workshop-linux` job repeats five race tests (`.github/workflows/ci.yml:265-272`).
- Goals:
  - No `workspace-hack` crate, no `.config/hakari.toml`, and no `cargo-hakari` in any manifest, workflow, or tool source.
  - Every workspace crate compiles on its own with only the features it declares.
  - The Windows Workshop job runs its tests as one command.
  - The Linux Workshop job runs no test that the main Linux test job already runs.
  - The shipped gateway and Workshop behave at runtime as they do today. The exceptions are losing tokio `test-util` and losing crates the binary never used.
- Non-goals:
  - Merging commands in the `docs`, `clippy`, or `test` jobs.
  - Moving `workshop-server` and `workshop-server-api` into the workspace-wide `--all-features` runs.
  - Adding a standing CI check that compiles each crate alone.
- Success criteria:
  - `rg -n --hidden "hakari|workspace-hack|workspace_hack" --glob '!vibe/**' --glob '!.git/**'` from the repository root returns nothing, `Cargo.lock` included. Without `--hidden`, rg skips `.github/` and `.config/`.
  - The normal closure of `runvil-types` (`cargo tree --locked -p runvil-types -e normal --prefix none`) has no `tokio` line. `cargo tree -i tokio` can't express this: it exits with an error when tokio isn't in the crate's graph at all.
  - In the gateway's `cargo tree --locked -p gateway -e normal -f "{p} {f}" --prefix none` output, the tokio line has no `test-util`.
  - The Windows `check-workshop` job has exactly one test step, and that step still runs the five race tests on Windows.
  - `ci.yml` is green on the branch. A dispatched `release-workshop.yml` run builds all five targets.
- Constraints:
  - CI runs cargo with `--locked`, so `Cargo.lock` changes are committed with the change.
  - The Workshop desktop crate compiles only after the featureless gateway is built and staged as Tauri's `externalBin` sidecar (`.github/workflows/ci.yml:168-172`; `vibe/2026-10-11-5-easy-fixes.md:226`).
  - The gateway links exactly one rustls crypto backend, aws-lc-rs. The CI guard at `.github/workflows/ci.yml:353-371` stays.
  - Shipping targets: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and `aarch64-apple-darwin` (`.config/hakari.toml:15-21`). That file is removed, so this list is the record.
- Open questions: None

## Functional Specification

The change shows up in the build and CI behavior: which features each build compiles, which CI steps run, and which tests run on which platform. `workshop-server` loses its `headless` build shape, so it always bundles the UI and needs Node.js to build. The shipped binaries keep their runtime behavior. Hakari was quietly giving the gateway one runtime feature, OS proxy detection, and that feature is now declared explicitly.

- Actors and workflows:
  - A developer or agent running `cargo build`, `check`, or `test` with `-p <crate>` gets that crate's declared features, unified only with the other packages selected in the same command.
  - CI `check-workshop` (Windows), in order:
    - Build the featureless gateway.
    - Stage the sidecar.
    - Clippy over `workshop`, `workshop-server`, and `workshop-server-api`.
    - The one merged test command.
    - The `workshop-server` private-items docs.
    - Remove the sidecar.
    - Clean-tree check.
  - CI `check-workshop-linux`: build the featureless gateway, stage the sidecar, build `workshop`, remove the sidecar, then the clean-tree check.
  - CI `supply-chain`: `cargo deny`, `cargo audit`, and the ring guard.
- Inputs and outputs:
  - The merged Windows test command selects six packages: `workshop`, `workshop-server`, `workshop-server-api`, `workshop-workspace`, `gateway-api-discovery`, and `gateway`.
  - It runs every test of the four Workshop crates plus the five named race tests. Its exact text is in Technical Design.
- States and validation:
  - `workshop-server` has no `headless` feature. Its build script always bundles the UI, and its asset routes always serve the embedded bundle.
- Errors and recovery:
  - A crate that relied on a feature hakari supplied fails to compile alone. Fix it by declaring the feature on that crate's own dependency line, or on its dev-dependency line when only tests need it.
  - The merged Windows command may fail to compile the gateway's test targets under `--no-default-features --features gateway/test-fixtures`. If a few `cfg` gates fix it, add them. Otherwise move the gateway into a second command (`-p gateway --no-default-features --features test-fixtures --test it` with the gateway half of the filter) and report two commands.
- Security and privacy behavior:
  - The gateway keeps aws-lc-rs as its only rustls backend.
  - The Workshop stays free of `ring` while `tauri-plugin-updater` keeps `default-features = false` (`Cargo.toml:242-251`).
  - The gateway keeps honoring the Windows registry and macOS system proxy settings, because reqwest's `system-proxy` feature becomes explicit.
- Acceptance criteria:
  - The Windows job runs the same tests as today: all tests of `workshop`, `workshop-server`, `workshop-server-api`, and `workshop-workspace` (the Windows path-jail tests included), plus the five race tests.
  - The headless-only tests in `crates/workshop/server/src/routes/assets-headless-tests.rs` are deleted along with the feature.
  - The per-crate check sweep passes for every workspace member.
  - The shipped-feature diff has been reviewed. Every remaining loss is either expected or recorded as accepted in the commit message.

</product-contract>
<implementation-contract>

## Technical Design

Feature unification goes back to cargo's default: within one command, across the packages that command selects. The `workspace-hack` crate is removed, along with its config, every dependency edge to it, and the build-xtask rules and docs that name it. The Windows CI job's test runs become one nextest command whose filter reproduces today's coverage. The workspace reqwest pin gains `system-proxy`, so the gateway keeps its proxy behavior.

- Architecture:
  - Hakari folds dev-dependency features into the normal dependencies of every member. The evidence is tokio `test-util`, which is listed as normal in the hack and only ever requested by dev-dependencies (Product Requirements). Without hakari, a feature reaches a crate only through its own manifest or through the other packages in the same cargo command.
  - Dependency features still unify across all packages selected in one command. This is the cargo behavior that lets the merged Windows command share one build (Cargo reference, features page, "Feature unification").
- Modules and interfaces:
  - build-xtask's product-boundary matrix (`crates/build-xtask/src/product.rs:15-23`, `:159-161`, `:231-236`, `:276-284`) changes as follows:
    - Outside their family, Harness crates may name only `runvil` and `runvil-plugin`. Any unaffiliated crate is a breach.
    - Plugin crates may name only `runvil-plugin`, `shared-*` crates, and outside libraries.
  - build-xtask's ceiling-wiring check loses its special case for a crate without a build script. The affected code is the `WORKSPACE_HACK` and `LINE_LIMIT` constants, the doc line, the branch, and `workspace_hack_violations` (`crates/build-xtask/src/tidy-wiring.rs:13-17`, `:25`, `:40-47`, `:193-213`). The `else` body of the branch stays.
  - workshop-server's asset layer becomes `EmbeddedAssets` alone. The `AssetServer` trait only existed to choose between `EmbeddedAssets` and `NoopAssets`, so it collapses into `EmbeddedAssets`, and `ui_asset` loses its server argument (`crates/workshop/server/src/assets.rs:66-141`, `crates/workshop/server/src/routes/assets.rs:11-24`). `NoopAssets` and its test `the_noop_asset_layer_serves_nothing` are deleted.
- File and public API changes:
  - Removed from the repository:
    - `crates/workspace-hack/`
    - `.config/hakari.toml`
    - `crates/workshop/server/src/routes/assets-headless-tests.rs`
  - Root `Cargo.toml`:
    - Remove the `workspace-hack` workspace dependency and its comment (`:75-76`).
    - Add `system-proxy` to the reqwest pin (`:89`) with a one-line reason.
    - Reword the `tauri-plugin-updater` comment (`:242-251`) to say its `rustls-tls` default would put `ring` into the Workshop binary.
  - Member manifests: delete the line `workspace-hack.workspace = true` from every member `Cargo.toml` under `crates/` that has it. There are 49, each with no adjoining comment; `rg -l '^workspace-hack\.workspace = true' crates` lists them.
  - Comments that name hakari or the hack:
    - `crates/build-ceiling/Cargo.toml:9-12`
    - `crates/runvil-internal/vfs/Cargo.toml:9-12`
    - `crates/build-xtask/src/engine_deps.rs:22-23`
    - `crates/build-xtask/src/test_support_leak.rs:34-35`
    - `crates/build-xtask/src/harness_bans.rs:11-12`
  - `## Invariants` docs that list `workspace-hack` as an allowed dependency:
    - `crates/runvil-plugin/src/lib.rs:22-23`
    - `crates/plugin-mcp/src/lib.rs:57-58`
    - `crates/plugin-user-input/src/lib.rs:68-69`
    - `crates/plugin-web/src/lib.rs:48-49`
    - `crates/runvil-internal/types/src/lib.rs:31`
  - build-xtask tests:
    - `crates/build-xtask/src/product/harness_tests.rs`: delete `a_harness_crate_depending_on_workspace_hack_passes` (`:53-67`) and update the expected messages at `:101` and `:122-124`.
    - `crates/build-xtask/src/product/plugin_tests.rs`: update the fixture at `:19`, `PLUGIN_RULE` at `:29`, and the passing test at `:32-39`.
    - `crates/build-xtask/src/tidy-wiring.rs`: update the fixture at `:248` and `:265-266`, and delete the test at `:337-346`.
  - workshop-server:
    - `Cargo.toml:65-68`: the `headless` feature and its comment.
    - `build.rs`: the `CARGO_FEATURE_HEADLESS` branch (`:17-19`), `empty_asset_dir` (`:36-51`), and the module-doc sentences about headless (`:7-10`).
    - `src/assets.rs`.
    - `src/routes/assets.rs`: the test-module gates at `:180-186` become plain `#[cfg(test)]`.
    - `src/serve-tests.rs:58-66`: the UI index assertion becomes unconditional.
  - `.github/workflows/ci.yml`:
    - `supply-chain`: drop `cargo-hakari` from the install list (`:342`), delete the `cargo hakari verify` step (`:350-351`), and drop "or a hakari regeneration" from the ring-guard comment (`:355-356`).
    - `check-workshop`: replace the steps at `:182-196` and `:203-210` with the merged step below.
    - `check-workshop-linux`: delete the race-test step (`:265-272`).
  - `Cargo.lock`: only the `workspace-hack` package and its dependency edges are removed. No versions change.
  - Crate manifests that relied on hakari gain the explicit features the per-crate sweep finds.
  - No public Rust API changes. The facade listing `crates/runvil/public-api.txt` is unaffected.
- Merged Windows test step, replacing the four steps named above:

  ```yaml
  # One build runs every Windows-only test: the Workshop crates' suites
  # (the workspace jail's Windows paths included) and the gateway's
  # process-ownership races, the only tests the filter selects from the
  # gateway and gateway-api-discovery suites.
  - name: Test (Windows)
    shell: bash
    run: >-
      cargo nextest run --locked --no-default-features --features gateway/test-fixtures
      -p workshop -p workshop-server -p workshop-server-api -p workshop-workspace
      -p gateway-api-discovery -p gateway
      -E 'package(workshop) | package(workshop-server) | package(workshop-server-api)
      | package(workshop-workspace)
      | (package(gateway-api-discovery) & test(a_process_lifetime_lease_recovers_after_its_owner_is_terminated))
      | (package(gateway) & binary(it) & (test(simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff)
      | test(workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot)
      | test(a_direct_launch_recovers_the_lease_from_a_terminated_owner)
      | test(a_silent_process_lease_owner_causes_a_bounded_console_only_failure)))'
  ```

  - **Feature flags.** `--no-default-features` turns off defaults for every selected package, and `gateway/test-fixtures` enables `test-fixtures` on the gateway alone (Cargo reference, features page, "Command-line feature options"). The four Workshop crates have empty defaults (`crates/workshop/desktop/Cargo.toml:59-60`, `crates/workshop/server/Cargo.toml:64`, `crates/workshop/server-api/Cargo.toml:9-10`, `crates/workshop/workspace/Cargo.toml:9-10`). `gateway-api-discovery` has no `default` at all (`crates/gateway-api-discovery/Cargo.toml:28-32`).
  - **workshop-workspace.** Its only feature is `test-fixtures`, which its own dev-dependency on itself already enables (`crates/workshop/workspace/Cargo.toml:15`, `:36`). So the old `--all-features` run changed nothing.
  - **Filter matching.** In nextest, `package()` and `binary()` default to glob matching, which needs the full name to match, and `test()` defaults to substring matching (nextest filterset reference, "Default matchers"). `package(workshop)` therefore matches only `workshop`.
  - **No sidecar needed.** The race tests start the gateway through `CARGO_BIN_EXE_runvil-gateway` (`crates/gateway/app/tests/it/boot/lease.rs:31`, `:62`; `crates/gateway/app/tests/it/support/process.rs:84`). The discovery test re-launches its own test executable (`crates/gateway-api-discovery/tests/it/main.rs:201`).
  - **What gets built.** The gateway's only test targets are its library unit tests and `it`; it has no examples or benches (`crates/gateway/app/Cargo.toml`). Its feature-specific test modules are gated, for example `crates/gateway/app/src/boot.rs:391-392` and `crates/gateway/app/tests/it/main.rs:38-45`. `crates/gateway/app/src/main-logging-tests.rs` gates per item.
- Data, persistence, failure, security, and privacy constraints:
  - **Proxy behavior.**
    - reqwest's `system-proxy` feature enables hyper-util's `client-proxy-system` (reqwest 0.13.5 `Cargo.toml:76-77`).
    - hyper-util's `Matcher::from_system()` always reads `ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`. The feature adds only the macOS system proxy configuration and the Windows registry proxy settings (hyper-util `src/client/proxy/matcher.rs:86-111`, `:228-249`).
    - A reqwest client with no explicit proxy configuration installs that system matcher (reqwest `src/async_impl/client.rs:313-314`, `:424-427`; reqwest `src/proxy.rs:514-526`).
  - **Gateway clients.**
    - The gateway's cloud, artifact, HF, and STT-config clients set no proxy: `crates/gateway/protocol/src/http_util.rs:25-65`, `crates/gateway/local/src/artifacts.rs:272`, `crates/gateway/local/src/runtime/hf_sidecar.rs:37`, `crates/gateway/cloud-providers/src/main.rs:110`, and `crates/gateway/config/src/config/stt.rs:470`.
    - Only loopback clients call `.no_proxy()`: `crates/gateway/local/src/server.rs:388-389`, `crates/gateway/local/src/dialect.rs:251-252`, and `crates/gateway/local/src/chat_templates/live.rs:246-247`.
    - No workspace manifest asks reqwest for `system-proxy`. The pin at `Cargo.toml:89` lacks it, and the per-crate reqwest lines add only `blocking` (`crates/gateway/local/Cargo.toml:17`; dev-only in `crates/gateway/config/Cargo.toml:20`) and `gzip`/`brotli` (`crates/plugin-web/Cargo.toml:17`). The gateway gets it today only through the hack (`crates/workspace-hack/Cargo.toml:53`).
  - **Test-only features.** Once hakari is removed, tokio `test-util` is compiled only into builds that include dev-dependencies.

</implementation-contract>
<verification-contract>

## Testing Plan

Beyond the usual gates, three checks show the change is correct. Every crate compiles alone. The shipped binaries' feature sets differ from today's only in expected or accepted ways. And the merged Windows command runs the same tests as the five commands it replaces. Code that only builds on Linux or macOS is covered by CI and a dispatched release build.

- Unit:
  - `cargo test -p build-xtask`, covering the updated product-boundary and ceiling-wiring tests.
  - `cargo nextest run --locked -p workshop-server` after the `headless` removal.
- Integration and end-to-end:
  - **Feature baseline and diff.** Before any manifest change, save as scratch the sorted, de-duplicated output of `cargo tree --locked -p <pkg> -e normal --target <triple> -f "{p} {f}" --prefix none`, for `gateway` and `workshop` on each of the five shipping targets. After the change, rerun the same commands and diff.
    - Expected: tokio loses `test-util`, and crates the binary never uses disappear.
    - Restore: anything that changes runtime behavior.
    - Accept: losses that only affect performance or API surface, such as reqwest `gzip` and `brotli` in the gateway. List them in the commit message.
  - **Per-crate sweep.** With the sidecar staged, run `cargo check --locked -p <m>` and `cargo check --locked -p <m> --all-targets --all-features` for each member `<m>` named by `cargo metadata --no-deps --format-version 1`.
  - **Merged Windows command.** Run it locally with the sidecar staged. `cargo nextest list` with the same arguments must show the five race tests and every test of the four Workshop crates.
  - **Cross-platform build.** Dispatch `.github/workflows/release-workshop.yml` on the branch. A dispatched run builds the gateway and the Workshop for all five targets. It uploads only temporary Actions artifacts and never reaches the publish job, which is gated to pushes of `workshop-v*` tags (`.github/workflows/release-workshop.yml:16-20`, `:71-87`, `:146-192`, `:483-485`).
- Regression, security, and performance:
  - `cargo tree --locked -p gateway -e normal -i ring` prints nothing, and so does the same command with `-p workshop`.
  - The normal closure (`cargo tree --locked -p <crate> -e normal --prefix none`) has no `tokio` line for `runvil-types`, `runvil-model-client`, `runvil-parser`, `runvil-lua`, `runvil-engine`, and `runvil`.
  - `cargo tree --locked -p gateway -e normal -i reqwest -f "{p} {f}"` lists `system-proxy`.
- Exit criteria: all of the following pass.
  - `cargo fmt --all --check`
  - `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, with the sidecar staged
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - The merged Windows command, with the sidecar staged
  - With `RUSTDOCFLAGS="-D warnings"`:
    - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
    - `cargo doc -p runvil --no-deps`
    - `cargo doc -p harness --no-deps`
    - `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
    - `cargo doc --locked --no-deps -p workshop-server --document-private-items`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo deny check`
  - The per-crate sweep, the regression checks above, and the reviewed feature diff
  - `rg -n --hidden "hakari|workspace-hack|workspace_hack" --glob '!vibe/**' --glob '!.git/**'` returns nothing
  - `ci.yml` green on the branch, and the dispatched `release-workshop.yml` run green

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - **Remove hakari entirely, not partially.**
    - Rationale: it compiled `test-util` into release binaries, put tokio, reqwest, hyper, and GUI crates under the Engine crates, and carried Workshop features into the gateway. It also needed a regenerate step on most manifest changes, and its rebuild savings were never measured. The heaviest CI steps are single `--workspace` commands, where cargo already unifies features (`.github/workflows/ci.yml:56`, `:85`, `:127`).
    - User's words: "I want all the workspace hack out, I want all the hakari out".
  - **Delete workshop-server's `headless` feature.**
    - Rationale: only its own CI step uses it (`.github/workflows/ci.yml:196`). It removes code through `#[cfg(not(feature = "headless"))]`, so it can never share a build with the default shape. CI always installs Node.js.
    - User's choice: "Delete `headless` - one test command".
  - **One test command in the Windows Workshop job.**
    - User's words: "I want all these different builds cut down to just 2 or preferably 1 if that's possible".
  - **Delete the Linux Workshop job's race-test step.**
    - Rationale: those tests don't use the sidecar, and the main Linux test job already runs them with `--all-features` (`.github/workflows/ci.yml:85`).
    - Follows from the user's request to cut the duplicate builds.
  - **Add `system-proxy` to the workspace reqwest pin.**
    - Rationale: the gateway keeps honoring Windows registry and macOS proxy settings, which today it gets only through the hack (Technical Design).
    - This follows the goal of unchanged shipped behavior; the user didn't state it.
  - **Fix hakari-masked features on each crate's own dependency line, not on the workspace pin.**
    - Rationale: this matches the repository's per-crate feature pattern, for example `crates/gateway/local/Cargo.toml:17`.
  - **Run the remote checks through a `remove-hakari` branch and a pull request into `master`.**
    - Rationale: `ci.yml` runs only on pushes to `master` or `main` and on pull requests (`.github/workflows/ci.yml:3-6`), and the run's commits shouldn't reach `master` before CI and the release build pass.
    - User's choice: "Push a `remove-hakari` branch and open a PR to run CI, then dispatch the release build on that branch".
  - **Collapse `AssetServer` into `EmbeddedAssets`, and delete `NoopAssets` along with its test.**
    - Rationale: with `headless` gone, the trait has one implementation and `NoopAssets` exists only to test itself.
- Rejected alternatives:
  - **List the Engine crates under hakari's `[final-excludes]`.**
    - Reason: it fixes the Engine closure but leaves `test-util` in the shipped binaries.
    - Revisit: none, since hakari is removed.
  - **Exclude tokio and its HTTP stack from hakari's output.**
    - Reason: it breaks easily on regeneration, gives up most of what hakari shares, and keeps the regenerate chore.
    - Revisit: none.
  - **Keep `headless` as a second test command, or turn it into an additive `ui` feature.**
    - Reason: either way it is a second build shape. The user chose one command.
    - Revisit: if building `workshop-server` without Node.js becomes a real need.
  - **Keep the gateway race tests as their own command.**
    - Reason: the merged command covers them.
    - Revisit: kept as the fallback in Functional Specification, Errors and recovery.
  - **Run the whole gateway and gateway-api-discovery suites on Windows.**
    - Reason: that grows coverage beyond today's.
    - Revisit: if Windows-specific gateway failures appear.
- Assumptions, risks, and notes:
  - **Risk: features hakari was quietly supplying.** Some per-crate builds break where a crate uses a feature it never declared; the sweep finds them. The local Windows sweep doesn't cover code that only builds on Linux or macOS. That code is covered by `ci.yml` and the dispatched release build.
  - **Risk: gateway test compile under the merged command.** It compiles the gateway's library unit tests under `--no-default-features --features gateway/test-fixtures` on Windows, a shape CI has never built. Feature-specific test modules are gated, so failures should be few. The fallback is two commands.
  - **Risk: shared dependency features.** One command with six packages unifies dependency features across all of them, so Workshop tests see the gateway's dependency features and the reverse. The hack already gave every crate the union, so no test sees fewer features than today.
  - **Risk: parallel race tests.** The race tests now run alongside the Workshop tests in one nextest run. They already run alongside the whole suite in the Linux test job (`.github/workflows/ci.yml:85`).
  - **Assumption: cross-target resolution.** `cargo tree --target <triple>` resolves on Windows for targets whose standard library isn't installed. If it doesn't, compute the baseline on matching CI runners, or later from the commit before the change.
  - **Note: accepted losses.** Feature losses accepted after the diff are listed in the commit message.
  - **Note: history stays.** The `vibe/` records that mention hakari are history and stay unchanged.
  - **Note: macOS coverage.** `ci.yml` has no macOS job. macOS builds run only in `.github/workflows/release-workshop.yml` and `.github/workflows/nightly.yml`.
  - **Note: ring in the Workshop.** It stays out only while `tauri-plugin-updater` keeps `default-features = false`, so the pin and its comment stay.

### Deferred and Out of Scope

- **Deferred: put `workshop-server` and `workshop-server-api` into the workspace-wide `--all-features` clippy, test, and docs runs.** With `headless` gone, nothing blocks this. Revisit after this change lands, since it touches the documented verification commands, `.githooks/pre-push:12`, and three CI jobs.
- **Out of scope:** merging commands in the `docs`, `clippy`, and `test` jobs.
- **Out of scope:** a standing CI check that compiles each crate alone.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace `default-members` is `crates/gateway/app` only); the Workshop desktop app builds explicitly with `cargo build --locked -p workshop`. Crate build scripts bundle the UIs, so a fresh clone needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first (both `node_modules` are present locally).
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>` for any crate outside the Workshop trio (drop `--all-features` for `workshop`, `workshop-server`, `workshop-server-api`); one integration target: `cargo test --locked -p <crate> --test <it|suite> <test_name>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`; for the Workshop trio `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; structural checks `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs`. UI suites: `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` (PowerShell: set `$env:CARGO_BUILD_WARNINGS = "deny"`), plus the headless gate `cargo check -p gateway --no-default-features`. Supply chain (CI `supply-chain` job): `cargo deny check`, `cargo audit`, `cargo hakari verify`, and the `cargo tree --locked -p gateway -e normal -i ring --depth 0` single-rustls-backend check. Locally installed: cargo-nextest 0.9.128, cargo-hakari 0.9.38, cargo-deny 0.20.2, Node 24.
- Formatter check command: `cargo fmt --all --check` (also the `.githooks/pre-commit` hook).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus `RUSTDOCFLAGS="-D warnings" cargo doc -p runvil --no-deps` (facade, default features). CI also runs `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (nightly pinned in `crates/build-xtask/src/api/toolchain.rs`).
- Test placement and naming conventions:
  - Unit tests sit in a kebab sibling `foo-tests.rs` wired from `foo.rs` with `#[cfg(test)]`, `#[path = "foo-tests.rs"]`, `mod tests;`. A module with three or more test files moves them into `src/<module>/tests/` (for example `runvil-internal/engine/src/execute/tests/`).
  - Integration tests use one target per crate, `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`harness`, `runvil`), declaring one `mod` per topic file; shared helpers go in `tests/common/`, data in `tests/fixtures/`, prompt fixtures in `tests/prompts/`. A few crates keep flat target files instead (`gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-cloud-providers`, `build-workshop`).
  - Test functions are snake_case sentences stating the behavior, such as `a_policy_refuses_an_undeclared_path_before_the_watcher_sees_it`.
  - `.config/nextest.toml` defines `default` and `ci` profiles and a `heavy` test group for the STT crates.
  - JavaScript tests are `node --test` `.mjs` files: `crates/workshop/ui/test/`, `tools/*.test.mjs`.
- Directory map:
  - `crates/runvil` is the Engine facade; `crates/runvil-internal/` (manifestless container) holds the private Engine crates `types`, `engine`, `lua`, `parser`, `vfs`, `model-client`; `crates/runvil-plugin` is the Plugin API.
  - `crates/harness`, `crates/harness-internal/runner`, and `crates/harness-gateway-client` are the Harness.
  - `crates/plugin-web`, `crates/plugin-mcp`, `crates/plugin-user-input` are linked Plugins.
  - `crates/gateway/` (container) holds the gateway family: `app` (the `gateway` crate), `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and `stt/{api,engine,backend-whisper,whisper-ffi}`. `crates/gateway-api-types` and `crates/gateway-api-discovery` are the gateway's cross-family wire types and discovery.
  - `crates/workshop/` (container) holds the Workshop Host: Rust crates `desktop` (the Tauri `workshop` crate), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, `run-log`, `agents`, and an npm workspace (`ui`, `look`, `platform`) rooted at `crates/workshop/package.json`.
  - `crates/shared-error-source` and `crates/shared-loopback` are shared Rust crates; `crates/shared-ui` is a TypeScript and CSS package excluded from the Cargo workspace.
  - `crates/build-*` are tooling: `build-xtask` (`cargo xtask` alias: structural checks, API surface, site), `build-workshop` (`cargo workshop` alias: build orchestrator), `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`.
  - `crates/workspace-hack` is the cargo-hakari feature-unification crate (generated `### BEGIN/END HAKARI SECTION` in its `Cargo.toml`, one-line `src/lib.rs`), configured by `.config/hakari.toml`. Its footprint: root `Cargo.toml` (`workspace-hack` entry in `[workspace.dependencies]` with its comment, and the `tauri-plugin-updater` comment), 49 member manifests with `workspace-hack.workspace = true`, explanatory comments in the `build-ceiling` and `runvil-vfs` manifests, `.github/workflows/ci.yml` (`supply-chain` job installs cargo-hakari and runs `cargo hakari verify`; the ring-check comment names hakari), `build-xtask` (`WORKSPACE_HACK` allowances in `product.rs` with `product/harness_tests.rs` and `product/plugin_tests.rs`, the `WORKSPACE_HACK` line-limit check and its tests in `tidy-wiring.rs`, doc comments in `engine_deps.rs`, `harness_bans.rs`, `test_support_leak.rs`), and crate docs in `runvil-plugin`, `plugin-web`, and `runvil-types` `lib.rs`.
  - Root config: `Cargo.toml`, `Cargo.lock`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml` (cargo-dist); `.cargo/config.toml` (rust-lld and static CRT on Windows MSVC, `xtask` and `workshop` aliases); `.config/` (`hakari.toml`, `nextest.toml`); `.github/workflows/` (`ci.yml` plus release, nightly, site, and smoke workflows); `.githooks/` (pre-commit fmt; pre-push docs-claims, headless check, clippy, cargo deny); `.cursor/rules/` (Workshop rules).
  - Non-code: `guide/` (user guide sources), `prompts/` (example prompts), `tools/` (Node scripts for gateway sidecar staging and TTS), `images/`, `vibe/` (plan archive), `local/` (gitignored operator config), `cabinet/`.
- Component boundaries (normal dependencies, from `cargo metadata`):
  - Engine: `runvil-types` and `runvil-vfs` are leaves; `runvil-model-client` uses `types`; `runvil-lua` uses `model-client`, `types`, `vfs`; `runvil-parser` uses `lua`, `types`; `runvil-engine` uses all five; the `runvil` facade re-exports all six. `runvil-plugin` uses `types` and `vfs`. No Engine crate depends on the Harness or a Host.
  - Plugins: each `plugin-*` crate depends only on `runvil-plugin` (rule allows `shared-*`, `workspace-hack`, and outside libraries).
  - Harness: `harness-runner` uses `runvil` and `runvil-plugin`; `harness` adds `harness-runner`; `harness-gateway-client` uses `harness`, `runvil`, `plugin-web`. Outside their family Harness crates may name only `runvil`, `runvil-plugin`, and `workspace-hack`.
  - Gateway: `gateway-api-types` is a leaf; `config` and `progress` use it; `protocol` uses `api-types` and `config`; `routing` uses `config` and `protocol`; `local` uses `config`, `progress`, `protocol`, `routing`; `stt/engine` and `stt/whisper-ffi` are leaves, `backend-whisper` uses them and `progress`, `stt/api` (`gateway-stt`) uses `config`, `local`, `progress`, and both STT crates; `web-search` uses `config` and `protocol`; `logging` uses `gateway-api-discovery`; `config-ui` uses `shared-loopback`; `cloud-providers` (a library plus the `shared-cloud-providers` binary) has no workspace dependents. The `gateway` app sits on top of the family.
  - Workshop: `workshop-protocol` and `workshop-support` are leaves; `registry` uses `protocol`; `menu`, `status`, `user-state`, `workspace` use `protocol`, `registry`, `support` (the last two also `shared-error-source`); `workshop-gateway` uses those plus `gateway-api-types` and `gateway-api-discovery` (no other gateway crate); `run-log` uses `harness`; `agents` uses `harness`, `runvil`, `plugin-user-input`; `workshop-server` sits over every Workshop crate plus the Harness, `harness-gateway-client`, all three Plugins, `runvil`, `runvil-plugin`; `workshop-server-api` wraps the server; `workshop` (desktop) uses `workshop-server-api` and `gateway-api-discovery`.
  - `build-*` and `shared-*` crates depend on no workspace crate. `workspace-hack` depends on no workspace crate and is depended on by every member except itself, `runvil-vfs`, and `build-ceiling` (the `final-excludes`).
  - `cargo test -p build-xtask` enforces product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary:
  - Every third-party dependency is declared once in root `[workspace.dependencies]`, usually with a comment explaining the pin or trimmed features, and members inherit it in dotted style (`name.workspace = true`); every crate has `[lints] workspace = true`.
  - Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used`, `expect_used`, `allow_attributes`, `allow_attributes_without_reason` deny, `unsafe_code` deny, `missing_docs` warn, all promoted by `CARGO_BUILD_WARNINGS=deny`.
  - Crate roots open with `//!` docs stating purpose and allowed dependencies; 27 carry a `## Invariants` section.
  - Source directories are flat; one or two related files become kebab siblings wired with `#[path]`, three or more become a subdirectory.
  - The capitalized terms Engine, Harness, Host, and Plugin have fixed meanings, enforced by `crates/workshop/ui/test/docs-claims.mjs` across `AGENTS.md` files, crate docs, and `.cursor/rules`; nested `AGENTS.md` files add per-crate rules.
  - Comments explain only non-obvious constraints; behavior changes ship with tests; error messages are written for model consumption.
  - CI fails if any build dirties the tree, and the `ci-green` job must list every job in `needs:`.

</project-survey>
<execution-plan>

## Execution Instructions

Six steps in two components. Each step is one commit that leaves its own tests passing. The work items map to steps this way: deleting `headless` is Step 1, the CI change is Step 2, the hakari removal and the per-crate sweep are Step 3 (one commit), the baseline and the feature diff are Step 4, the build-xtask cleanup is Step 5, and verification is Step 6.

Test commands are written for Windows PowerShell 5.1, the shell of the local checkout. It drops double quotes nested inside single-quoted arguments to native programs, so no command nests them. A command that sets `CARGO_BUILD_WARNINGS` or `RUSTDOCFLAGS` clears it again, so the setting never reaches a later command. Every command that builds `workshop` runs between the sidecar stage and remove commands, because the desktop crate compiles only with the featureless gateway staged as Tauri's `externalBin`.

Components, in dependency order:

1. One Workshop test build (Steps 1-2). It comes first because the per-crate sweep in Step 3 runs `--all-features` on `workshop-server`, which must no longer turn on the `headless` shape, and because it needs nothing from the hakari removal. Two pieces, built in sequence:
   - The single build shape (Step 1). `headless` goes together with its one CI step, so no commit leaves CI asking for a deleted feature.
   - The merged command (Step 2). The remaining Windows test steps become one nextest command, and the duplicate Linux race step goes. It follows Step 1 because one command can't also build the `headless` shape.
2. Hakari removal (Steps 3-6). It comes second because its sweep needs Step 1, and its last step reruns Step 2's command against the final manifests. Four pieces, built in sequence:
   - Removal and sweep (Step 3), built jointly in one commit. Removing the hack breaks exactly the crates the sweep fixes, so neither half compiles alone.
   - Feature restoration (Step 4). It follows Step 3 because it diffs the shipped closures against the parent of Step 3's commit.
   - Rule and doc cleanup (Step 5). It follows Step 3 because `workspace_respects_the_product_boundary` (`crates/build-xtask/src/product/tests.rs`) and `every_workspace_crate_runs_the_ceiling_from_its_build_script` (`crates/build-xtask/src/tidy-tests.rs`) check the live workspace and fail while 49 members still name `workspace-hack`. It doesn't depend on Step 4.
   - Cross-platform proof (Step 6). It comes last because it runs every exit criterion against the finished change, including CI and a dispatched release build.

<step-1>

### Step 1: Delete workshop-server's `headless` feature [completed]

- Component: One Workshop test build
- Tests:
  - `cargo nextest run --locked -p workshop-server`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets; Remove-Item Env:CARGO_BUILD_WARNINGS`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps -p workshop-server --document-private-items; Remove-Item Env:RUSTDOCFLAGS`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`
  - `cargo fmt --all --check`
  - `if (rg -n headless crates/workshop/server) { throw 'workshop-server still names headless' }`
  - `if (rg -n 'features headless' .github/workflows/ci.yml) { throw 'ci.yml still runs the headless shape' }`
- Commit: one commit with the summary `Delete workshop-server's headless feature`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: Technical Design, the asset-layer entry of "Modules and interfaces" and the `crates/workshop/server/src/routes/assets-headless-tests.rs` and workshop-server entries of "File and public API changes".
- Before editing: read the root `AGENTS.md`, `crates/workshop/AGENTS.md`, and `crates/workshop/server/AGENTS.md`.

Artifacts:

- `crates/workshop/server/Cargo.toml`: delete `headless = []` and its three-line comment (lines 65-68). `default = []` and `test-fixtures` stay.
- `crates/workshop/server/build.rs`: delete the `CARGO_FEATURE_HEADLESS` branch (lines 17-19) and `empty_asset_dir` (lines 36-51). The module doc ends at the `npm ci` sentence, without the headless sentences (lines 7-10). `main` runs `build_ceiling::check()` and then always builds the UI.
- `crates/workshop/server/src/assets.rs`:
  - Delete the `AssetServer` trait (lines 66-78), `NoopAssets` and its impl (lines 106-120), and the test `the_noop_asset_layer_serves_nothing` (lines 189-201).
  - `EmbeddedAssets` keeps `get` and `manifest` as inherent associated functions with no `self` parameter (`EmbeddedAssets::get(path)`, `EmbeddedAssets::manifest()`). The struct is a unit type, and clippy's pedantic `unused_self` is denied.
  - `ui_asset(path, content_type, cache)` loses its `server: &dyn AssetServer` parameter and reads through `EmbeddedAssets::get`.
  - Drop every `#[cfg(not(feature = "headless"))]` and the headless sentences of the module doc and the `UiAssets` doc. The test-module gate (line 143) becomes `#[cfg(test)]`. The three traversal tests and `the_cache_policies_render_their_header_values` stay.
- `crates/workshop/server/src/routes/assets.rs`: delete `asset_server()` (lines 11-24) and the `AssetServer` import. Each handler calls `assets::ui_asset` without a server argument, and `logical_asset` reads `assets::EmbeddedAssets::manifest()`. The `routes` doc names `EmbeddedAssets` where it now says `AssetServer`. The test wiring (lines 180-186) becomes `#[cfg(test)]`, `#[path = "assets-tests.rs"]`, `mod tests;`, with no `headless_tests` module.
- Delete `crates/workshop/server/src/routes/assets-headless-tests.rs`.
- `crates/workshop/server/src/serve-tests.rs`: in `the_server_boots_and_serves_the_ui_with_an_unreachable_gateway`, drop the headless comment and the `#[cfg(not(feature = "headless"))]` wrapper (lines 58-67), so the `/` request and its `200 OK` assertion always run.
- `.github/workflows/ci.yml`, job `check-workshop`: delete the `Test (workshop-server headless)` step and its three-line comment (lines 192-196). The step is the feature's only user (Decision Record), so it goes in this commit. Step 2 replaces the job's other test steps.

Behavior:

- `workshop-server` has no `headless` feature. Its build script always bundles the UI with esbuild, so building it needs Node.js 22 and `npm ci --prefix crates/workshop`, which CI always installs. Its asset routes always serve the embedded bundle.
- The headless-only tests are deleted with the feature, by design. Every other `workshop-server` test still runs and passes.
- No public Rust API changes. Every item touched is `pub(crate)`.

</step-1>

<step-2>

### Step 2: Run the Windows Workshop tests as one nextest command [completed]

- Component: One Workshop test build
- Tests:
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `cargo nextest run --locked --no-default-features --features gateway/test-fixtures -p workshop -p workshop-server -p workshop-server-api -p workshop-workspace -p gateway-api-discovery -p gateway -E 'package(workshop) | package(workshop-server) | package(workshop-server-api) | package(workshop-workspace) | (package(gateway-api-discovery) & test(a_process_lifetime_lease_recovers_after_its_owner_is_terminated)) | (package(gateway) & binary(it) & (test(simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff) | test(workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot) | test(a_direct_launch_recovers_the_lease_from_a_terminated_owner) | test(a_silent_process_lease_owner_causes_a_bounded_console_only_failure)))'`
  - `cargo nextest list --locked --no-default-features --features gateway/test-fixtures -p workshop -p workshop-server -p workshop-server-api -p workshop-workspace -p gateway-api-discovery -p gateway -E 'package(workshop) | package(workshop-server) | package(workshop-server-api) | package(workshop-workspace) | (package(gateway-api-discovery) & test(a_process_lifetime_lease_recovers_after_its_owner_is_terminated)) | (package(gateway) & binary(it) & (test(simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff) | test(workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot) | test(a_direct_launch_recovers_the_lease_from_a_terminated_owner) | test(a_silent_process_lease_owner_causes_a_bounded_console_only_failure)))'`
  - `cargo nextest list --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo nextest list --locked -p workshop-workspace --all-features`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`
  - `cargo fmt --all --check`
- Commit: one commit with the summary `Run the Windows Workshop tests as one nextest command`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: Technical Design, the `check-workshop` and `check-workshop-linux` entries of "File and public API changes", the merged Windows test step, and its notes on feature flags, `workshop-workspace`, filter matching, the sidecar, and what gets built.
- Before editing: read the root `AGENTS.md`, `crates/workshop/AGENTS.md`, `crates/gateway/app/AGENTS.md`, and `crates/gateway-api-discovery/AGENTS.md`.

Artifacts:

- `.github/workflows/ci.yml`, job `check-workshop`: replace `Test (workshop, concurrent via nextest)`, `Test (workshop-workspace, Windows jail)` with its comment, and `Test Gateway process ownership races` (lines 182-190 and 203-210 before Step 1) with the `Test (Windows)` step exactly as written in Technical Design, comment and `shell: bash` included. It goes right after `Clippy (workshop)`.
- `.github/workflows/ci.yml`, job `check-workshop-linux`: delete `Test Gateway process ownership races` (lines 265-272 before Step 1).
- Gateway test code, only if the merged command fails to compile the gateway's library unit tests or its `it` target under `--no-default-features --features gateway/test-fixtures`: add the few missing `#[cfg(feature = "...")]` gates in `crates/gateway/app/src/` or `crates/gateway/app/tests/it/`, in the style of `crates/gateway/app/src/boot.rs:391-392` and `crates/gateway/app/tests/it/main.rs:38-45`. `crates/gateway/app/src/main-logging-tests.rs` gates per item.

Behavior:

- `check-workshop` on Windows runs, in order: Build featureless Gateway, Stage Gateway sidecar, Clippy (workshop), Test (Windows), Docs (workshop-server, private items), Remove Gateway sidecar, Clean tree. It has exactly one test step.
- `check-workshop-linux` runs: Build featureless Gateway, Stage Gateway sidecar, Build (workshop, Linux), Remove Gateway sidecar, Clean tree. The main Linux `test` job already runs the five race tests with `--all-features` (`.github/workflows/ci.yml:85`), so Linux coverage doesn't change.
- The merged command runs the same tests as the Windows job ran before Step 1, minus the deleted headless-only tests:
  - every test of `workshop`, `workshop-server`, `workshop-server-api`, and `workshop-workspace`, including the workspace jail's Windows path tests (verbatim, UNC, junctions, alternate data streams, symlinks)
  - `gateway-api-discovery`'s `a_process_lifetime_lease_recovers_after_its_owner_is_terminated`
  - the gateway `it` tests `simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff`, `workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot`, `a_direct_launch_recovers_the_lease_from_a_terminated_owner`, and `a_silent_process_lease_owner_causes_a_bounded_console_only_failure`
  - no other gateway or `gateway-api-discovery` test
- List check: the merged `cargo nextest list` shows exactly those five race tests from the gateway packages. For each of the four Workshop crates, it shows the same tests as the two plain `cargo nextest list` commands in Tests.
- Fallback: if a few `cfg` gates don't fix the gateway compile, split the step into two commands in the same `run:` block. The first command keeps the four Workshop crates and `gateway-api-discovery` with their half of the filter. The second is `cargo nextest run --locked -p gateway --no-default-features --features test-fixtures --test it` with the gateway half of the filter. Report "two commands" in the step's return and the commit body.
- Accepted by the Decision Record: one six-package command unifies dependency features across all six, and the race tests run in parallel with the Workshop tests, as they already do in the Linux `test` job.

</step-2>

<step-3>

### Step 3: Remove hakari and declare the features it supplied [completed]

- Component: Hakari removal
- Tests:
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `foreach ($m in (cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json).packages.name) { cargo check --locked -p $m; if ($LASTEXITCODE) { throw "cargo check -p $m failed" }; cargo check --locked -p $m --all-targets --all-features; if ($LASTEXITCODE) { throw "cargo check -p $m --all-targets --all-features failed" } }`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`
  - `cargo test -p build-xtask`
  - `cargo deny check`
  - `if (rg -n --hidden 'hakari|workspace-hack|workspace_hack' --glob '*.toml' --glob '*.yml' --glob 'Cargo.lock' --glob '!vibe/**' --glob '!.git/**') { throw 'hakari remains in a manifest, workflow, or Cargo.lock' }`
- Commit: one commit with the summary `Remove cargo-hakari and the workspace-hack crate`, a body that names every feature the sweep added and the code that needs it, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: Technical Design, "Architecture", and these entries of "File and public API changes": the removed `crates/workspace-hack/` and `.config/hakari.toml`, the root `Cargo.toml` `workspace-hack` entry and `tauri-plugin-updater` comment, the member manifests, the `build-ceiling` and `runvil-vfs` manifest comments, the `supply-chain` job, `Cargo.lock`, and the manifests the sweep fixes.
- Before editing: read the root `AGENTS.md`.

Artifacts:

- Delete `crates/workspace-hack/` (its generated `Cargo.toml` and one-line `src/lib.rs`) and `.config/hakari.toml`. `.config/nextest.toml` stays.
- Root `Cargo.toml`:
  - Delete the `workspace-hack` entry of `[workspace.dependencies]` and its comment (lines 75-76).
  - In the `tauri-plugin-updater` comment (lines 243-250), replace the sentence "Through workspace-hack that feature would put `ring` back into every member, the gateway included." with one saying the plugin's `rustls-tls` default would put `ring` into the Workshop binary. The rest of the comment and `default-features = false, features = ["system-proxy", "zip"]` stay.
  - The reqwest pin is Step 4's.
- Delete the line `workspace-hack.workspace = true` from each of the 49 member manifests that `rg -l '^workspace-hack\.workspace = true' crates` lists. None has an adjoining comment.
- `crates/build-ceiling/Cargo.toml` (lines 9-12): the comment becomes "No dependencies of any kind: the six crates that may not name a build-* crate include src/lib.rs with #[path] from their build scripts, so it must compile on std alone." It loses "not even workspace-hack" and the `.config/hakari.toml` sentence.
- `crates/runvil-internal/vfs/Cargo.toml` (lines 9-12): keep the zero-dependency rule's first two sentences and drop the sentence beginning "workspace-hack is excluded".
- `.github/workflows/ci.yml`, job `supply-chain`: the install list becomes `tool: cargo-deny, cargo-audit` (line 342). Delete the `cargo hakari verify` step (lines 350-351). The ring-guard comment's parenthetical becomes "(through a reqwest bump or a Tauri plugin default)" (lines 355-356). The guard step itself (lines 353-371) is unchanged.
- `Cargo.lock`: refresh it once with `cargo update --workspace`, which updates only workspace members. `git diff HEAD -- Cargo.lock` must show only removed lines: the `workspace-hack` package block, the `"workspace-hack",` dependency entries, and any third-party package that only the hack reached. No line is added and no version changes. Name any removed third-party package in the commit body.
- Member manifests that relied on hakari gain the features the sweep finds.

Sweep:

- With the sidecar staged, run `cargo check --locked -p <m>` and `cargo check --locked -p <m> --all-targets --all-features` for each member `<m>` that `cargo metadata --no-deps --format-version 1` names. The Tests loop does this and stops at the first failure. 51 members remain once `workspace-hack` is gone.
- A failure means the crate used a feature hakari supplied. Declare it on that crate's own dependency line in its own `Cargo.toml`, for example `tokio = { workspace = true, features = ["macros"] }`, the pattern of `crates/gateway/local/Cargo.toml:17`. Use the dev-dependency line when only tests need it, and the matching `[target.'cfg(...)'.dependencies]` table when only target-specific code needs it. Never add it to the root `[workspace.dependencies]` pin (Decision Record). Rerun the loop until every member passes both checks.

Behavior:

- No `workspace-hack` crate, no `.config/hakari.toml`, and no `cargo-hakari` in any manifest, workflow, or `Cargo.lock`. The mentions left in build-xtask sources and crate docs are Step 5's.
- Every workspace crate compiles alone with only the features it declares. A `-p <crate>` command gets that crate's features, unified only with the other packages the same command selects.
- tokio `test-util` is compiled only into builds that include dev-dependencies.
- `cargo test -p build-xtask` still passes. Its `workspace-hack` allowances stay until Step 5 and no longer match any crate.
- CI runs cargo with `--locked`, so the refreshed `Cargo.lock` lands in this commit.
- Security: the gateway still links only aws-lc-rs, and the ring guard stays. `tauri-plugin-updater` keeps `default-features = false`, which is what keeps `ring` out of the Workshop.
- This Windows sweep doesn't cover code that only builds on Linux or macOS. Step 6 covers it through CI and the release build.

</step-3>

<step-4>

### Step 4: Restore the gateway's system proxy support and review the shipped feature diff [completed]

- Component: Hakari removal
- Tests:
  - `cargo check --locked -p gateway`
  - `cargo check -p gateway --no-default-features`
  - `$out = cargo tree --locked -p gateway -e normal -i reqwest -f "{p} {f}" --depth 0; if (-not ($out -match 'system-proxy')) { throw "the gateway's reqwest lacks system-proxy: $out" }`
  - `$out = cargo tree --locked -p gateway -e normal -f "{p} {f}" --prefix none | Select-String '^tokio v'; if ($out -match 'test-util') { throw 'the gateway still compiles tokio test-util' }`
  - `foreach ($p in 'gateway', 'workshop') { $out = cargo tree --locked -p $p -e normal -i ring; if ($LASTEXITCODE) { throw "cargo tree -p $p failed" }; if ($out) { throw "$p links ring: $out" } }`
  - `foreach ($c in 'runvil-types', 'runvil-model-client', 'runvil-parser', 'runvil-lua', 'runvil-engine', 'runvil') { $out = cargo tree --locked -p $c -e normal --prefix none; if ($LASTEXITCODE) { throw "cargo tree -p $c failed" }; if ($out -match '^tokio v') { throw "$c still reaches tokio" } }`
- Commit: one commit with the summary `Keep the gateway's system proxy support without hakari`, a body that lists every restored feature and every accepted loss with its reason, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: Technical Design, the root `Cargo.toml` reqwest-pin entry of "File and public API changes", and the "Proxy behavior", "Gateway clients", and "Test-only features" constraints.
- Before editing: read the root `AGENTS.md` and `crates/gateway/app/AGENTS.md`.

Artifacts:

- Root `Cargo.toml`, the reqwest pin (line 89): its features become `["json", "query", "rustls", "stream", "system-proxy"]`. Add one comment line saying `system-proxy` keeps the gateway honoring the Windows registry and macOS system proxy settings. This goes on the workspace pin by decision, unlike the sweep fixes.
- Crate manifests, only for a further runtime-relevant loss the diff shows: the lost feature on that crate's own dependency line.
- `Cargo.lock`, only when the pin change leaves it stale under `--locked`: refresh it with `cargo update --workspace`. No third-party version changes.
- Scratch that is never committed: the listings and diffs, saved as scratch outside the repository, and a temporary worktree at `$env:TEMP\rv-feature-baseline`.

Feature diff:

- Baseline tree: `$base = git rev-parse "$(git log -1 --format=%H -- crates/workspace-hack/src/lib.rs)^"` names the parent of Step 3's commit, the last tree with hakari. Check it out with `git worktree add --detach "$env:TEMP\rv-feature-baseline" $base`. Steps 1 and 2 don't change the gateway's or the Workshop's feature closure, so this stands in for a baseline taken before any manifest change.
- For `gateway` and `workshop` on each shipping target (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`), run `cargo tree --locked -p <pkg> -e normal --target <triple> -f "{p} {f}" --prefix none` twice: once with `--manifest-path "$env:TEMP\rv-feature-baseline\Cargo.toml"` and once in the working tree. Drop each line's trailing ` (*)` marker, sort, de-duplicate, and save the results as the scratch files `before-<pkg>-<triple>.txt` and `after-<pkg>-<triple>.txt`. Compare each pair with `git diff --no-index`.
- If `cargo tree --target <triple>` can't resolve a triple on Windows, stop and return blocked with the triple and the error. The plan's fallback is to compute that triple on a matching CI runner.
- Classify every difference:
  - Expected: tokio loses `test-util`, and crates the binary never uses disappear.
  - Restore: anything that changes runtime behavior. reqwest `system-proxy` in the gateway is already known.
  - Accept: losses that only affect performance or API surface, such as reqwest `gzip` and `brotli` in the gateway. List each one in the commit body.
  - A gained feature or crate is unexpected. Report it in the step's return.
- After restoring, rerun the after-listings so the final diff holds only expected and accepted losses. Then run `git worktree remove "$env:TEMP\rv-feature-baseline"`.

Behavior:

- The gateway keeps honoring the Windows registry and macOS system proxy settings. reqwest's `system-proxy` enables hyper-util's `client-proxy-system`. hyper-util's `Matcher::from_system()` always reads `ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`, and the feature adds only the macOS system configuration and the Windows registry settings. A reqwest client with no explicit proxy configuration installs that system matcher.
- The gateway's cloud, artifact, HF, and STT-config clients set no proxy (`crates/gateway/protocol/src/http_util.rs:25-65`, `crates/gateway/local/src/artifacts.rs:272`, `crates/gateway/local/src/runtime/hf_sidecar.rs:37`, `crates/gateway/cloud-providers/src/main.rs:110`, `crates/gateway/config/src/config/stt.rs:470`), so they pick it up. The loopback clients that call `.no_proxy()` are unaffected.
- No workspace manifest asked for `system-proxy` before this step. The gateway got it only through `crates/workspace-hack/Cargo.toml:53`.
- The shipped gateway and Workshop behave at runtime as before the change, apart from losing tokio `test-util` and crates the binary never used.
- The regression checks in Tests pass: no `ring` under the gateway or the Workshop, no tokio under the six Engine crates, `system-proxy` on the gateway's reqwest, and no `test-util` on the gateway's tokio.

</step-4>

<step-5>

### Step 5: Drop the workspace-hack allowances from build-xtask and the crate docs [completed]

- Component: Hakari removal
- Tests:
  - `cargo test -p build-xtask`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p build-xtask -p runvil-plugin -p plugin-mcp -p plugin-user-input -p plugin-web -p runvil-types --all-targets --all-features; Remove-Item Env:CARGO_BUILD_WARNINGS`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-plugin -p plugin-mcp -p plugin-user-input -p plugin-web -p runvil-types; Remove-Item Env:RUSTDOCFLAGS`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo fmt --all --check`
  - `if (rg -n --hidden 'hakari|workspace-hack|workspace_hack' --glob '!vibe/**' --glob '!.git/**') { throw 'hakari references remain' }`
- Commit: one commit with the summary `Drop the workspace-hack allowances from build-xtask`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: Technical Design, the build-xtask entries of "Modules and interfaces", and the comment, `## Invariants`, and build-xtask test entries of "File and public API changes".
- Before editing: read the root `AGENTS.md` and `crates/runvil-internal/types/AGENTS.md`.

Artifacts:

- `crates/build-xtask/src/product.rs`:
  - Module doc (lines 15-20): Harness crates outside their own family "may name only `runvil` and `runvil-plugin`, which rules out `build-*` and other unaffiliated crates". Plugin crates "may depend only on `runvil-plugin`, `shared-*` crates, and outside libraries".
  - Delete `WORKSPACE_HACK` and its doc (lines 159-161).
  - In `boundary_breach`, the `(Family::Harness, Family::Build)` message becomes `harness crates must not depend on build-* crates; outside their family they may name only runvil and runvil-plugin`. The `(Family::Harness, Family::Unaffiliated)` arm loses its `if dep.package != WORKSPACE_HACK` guard, and its message becomes `harness crates must not depend on unaffiliated crates; outside their family they may name only runvil and runvil-plugin`.
  - In `plugin_breach`, delete the comment at lines 276-277. The allow-list becomes `[PLUGIN_CONTRACT, package.package.as_str()]`, and the message becomes `plugin crates may depend only on runvil-plugin, shared-* crates, and outside libraries`.
- `crates/build-xtask/src/product/harness_tests.rs`: delete `a_harness_crate_depending_on_workspace_hack_passes` (lines 52-67). The expected texts at lines 101 and 122 become the two new Harness messages. The assertion message at line 124 says unaffiliated crates are closed to harness crates.
- `crates/build-xtask/src/product/plugin_tests.rs`: the module doc (lines 1-5) drops `workspace-hack`. `plugin_web_naming` drops its `("workspace-hack", "workspace-hack")` fixture crate (line 19). `PLUGIN_RULE` (line 29) becomes the new Plugin message. The passing test (lines 31-40) becomes `a_plugin_crate_naming_the_contract_and_a_shared_crate_passes`, without its `workspace-hack` dependency line.
- `crates/build-xtask/src/tidy-wiring.rs`:
  - Delete the `WORKSPACE_HACK` and `LINE_LIMIT` constants and their docs (lines 13-17).
  - The `ceiling_wiring_violations` doc drops "and that no `workspace-hack` file is over the limit" (lines 24-25).
  - The loop body becomes today's `else` body (lines 43-46), with no branch. Delete `workspace_hack_violations` (lines 192-213). `fs` stays imported for the manifest read, and `super::rust_files` keeps its other callers.
  - Tests: the `write_wired` doc drops "and a `workspace-hack` with no build script" (lines 247-248), and its last two lines (265-266) go. Delete `a_workspace_hack_file_over_the_limit_fails` (lines 337-346).
- Guard comments in `crates/build-xtask/src/engine_deps.rs` (lines 22-23), `crates/build-xtask/src/test_support_leak.rs` (lines 34-35), and `crates/build-xtask/src/harness_bans.rs` (lines 11-12): keep "The check reads declared dependencies, not the resolved graph." and drop the clause about `workspace-hack` unification. The sentences after it stay.
- `## Invariants` docs, reflowed:
  - `crates/runvil-plugin/src/lib.rs` (lines 22-23): "May depend on: `runvil-types` and `runvil-vfs` among workspace crates."
  - `crates/plugin-mcp/src/lib.rs` (lines 57-58), `crates/plugin-user-input/src/lib.rs` (lines 68-69), and `crates/plugin-web/src/lib.rs` (lines 48-49): "May depend on: `runvil-plugin`, `shared-*` crates, and outside libraries."
  - `crates/runvil-internal/types/src/lib.rs` (line 31): "May depend on: no workspace crate."

Behavior:

- Outside their family, Harness crates may name only `runvil` and `runvil-plugin`. Any unaffiliated crate is a breach, `workspace-hack` included.
- Plugin crates may name only `runvil-plugin`, `shared-*` crates, and outside libraries.
- The ceiling-wiring check holds every crate to a package-root `build.rs` that calls `build_ceiling::check()`, with no special case for a crate without a build script.
- The docs keep the fixed meanings of Engine, Harness, Host, and Plugin that `docs-claims.mjs` enforces.
- After this step, `rg -n --hidden "hakari|workspace-hack|workspace_hack" --glob '!vibe/**' --glob '!.git/**'` returns nothing, `Cargo.lock` included. The `vibe/` records stay unchanged as history.

</step-5>

<step-6>

### Step 6: Pass every exit criterion on Windows, Linux, and macOS [completed]

- Component: Hakari removal
- Tests:
  - `cargo fmt --all --check`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features; Remove-Item Env:CARGO_BUILD_WARNINGS`
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api; Remove-Item Env:RUSTDOCFLAGS`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps; Remove-Item Env:RUSTDOCFLAGS`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p harness --no-deps; Remove-Item Env:RUSTDOCFLAGS`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items; Remove-Item Env:RUSTDOCFLAGS`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo deny check`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets; Remove-Item Env:CARGO_BUILD_WARNINGS`
  - `cargo nextest run --locked --no-default-features --features gateway/test-fixtures -p workshop -p workshop-server -p workshop-server-api -p workshop-workspace -p gateway-api-discovery -p gateway -E 'package(workshop) | package(workshop-server) | package(workshop-server-api) | package(workshop-workspace) | (package(gateway-api-discovery) & test(a_process_lifetime_lease_recovers_after_its_owner_is_terminated)) | (package(gateway) & binary(it) & (test(simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff) | test(workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot) | test(a_direct_launch_recovers_the_lease_from_a_terminated_owner) | test(a_silent_process_lease_owner_causes_a_bounded_console_only_failure)))'`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps -p workshop-server --document-private-items; Remove-Item Env:RUSTDOCFLAGS`
  - `foreach ($m in (cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json).packages.name) { cargo check --locked -p $m; if ($LASTEXITCODE) { throw "cargo check -p $m failed" }; cargo check --locked -p $m --all-targets --all-features; if ($LASTEXITCODE) { throw "cargo check -p $m --all-targets --all-features failed" } }`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`
  - `$out = cargo tree --locked -p gateway -e normal -i reqwest -f "{p} {f}" --depth 0; if (-not ($out -match 'system-proxy')) { throw "the gateway's reqwest lacks system-proxy: $out" }`
  - `$out = cargo tree --locked -p gateway -e normal -f "{p} {f}" --prefix none | Select-String '^tokio v'; if ($out -match 'test-util') { throw 'the gateway still compiles tokio test-util' }`
  - `foreach ($p in 'gateway', 'workshop') { $out = cargo tree --locked -p $p -e normal -i ring; if ($LASTEXITCODE) { throw "cargo tree -p $p failed" }; if ($out) { throw "$p links ring: $out" } }`
  - `foreach ($c in 'runvil-types', 'runvil-model-client', 'runvil-parser', 'runvil-lua', 'runvil-engine', 'runvil') { $out = cargo tree --locked -p $c -e normal --prefix none; if ($LASTEXITCODE) { throw "cargo tree -p $c failed" }; if ($out -match '^tokio v') { throw "$c still reaches tokio" } }`
  - `if (rg -n --hidden 'hakari|workspace-hack|workspace_hack' --glob '!vibe/**' --glob '!.git/**') { throw 'hakari references remain' }`
- Commit: one commit holding the fixes that CI or the release build show are needed, plus the step's completion marker in the run's copy of this plan, with a body, `Design:` lines when code changes, and a `Plan: vibe/<file>` trailer naming that copy. When both runs pass with no change, the commit holds only the completion marker, and the step says so in its return.
- Contract items: Technical Design, the "Test-only features" constraint and the manifests the sweep fixes. The Testing Plan's exit criteria are copied below.
- Before editing: read the root `AGENTS.md`.

Artifacts:

- The `remove-hakari` branch on `origin` (`git@github.com:vinniefalco/runvil.git`), pushed with Steps 1-5, and a pull request from it into `master` opened with `gh pr create`. `ci.yml` runs only on pushes to `master` or `main` and on pull requests (`.github/workflows/ci.yml:3-6`), so the pull request is what runs it.
- A green `ci.yml` run for the pull request's head, followed with `gh run watch`.
- A dispatched `.github/workflows/release-workshop.yml` run on the same branch (`gh workflow run release-workshop.yml --ref remove-hakari`), also followed with `gh run watch`.
- Member manifests, only for a feature that Linux- or macOS-only code took from hakari: the feature goes on that crate's own dependency line, in the matching `[target.'cfg(...)'.dependencies]` table when only that target's code uses it, and never on the root pin.

Behavior:

- Every Tests command passes locally. Together they are the Testing Plan's exit criteria: the formatter, both clippy partitions, the featureless gateway check, the workspace test run, the merged Windows command with the sidecar staged, the five doc builds, build-xtask, the docs-claims scan, `cargo deny`, the per-crate sweep, the regression checks, and the reference scan.
- The reviewed feature diff is Step 4's commit body. This step doesn't redo it.
- `ci.yml` is green on the branch. The Windows `check-workshop` job has exactly one test step, and that step runs the five race tests on Windows.
- The dispatched `release-workshop.yml` run builds the gateway and the Workshop for all five targets and passes. A dispatched run uploads only temporary Actions artifacts and never reaches the publish job, which runs only on pushes of `workshop-v*` tags (`.github/workflows/release-workshop.yml:16-20`, `:71-87`, `:146-192`, `:483-485`).
- `ci.yml` has no macOS job, so macOS code is proven only by the release run (and `.github/workflows/nightly.yml`).
- When a fix lands, rerun the affected local Tests and push again, and both runs must be green on the fix commit.

</step-6>

</execution-plan>
