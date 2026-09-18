---
name: Single rustls backend
overview: Move the PromptForge workspace from reqwest 0.12 (ring) to reqwest 0.13 (aws-lc-rs, the backend Tauri and hf-hub already force into the tree) so the gateway links exactly one rustls crypto backend, guard that invariant in CI, and unify the toml pin on 1.x.
todos:
  - id: manifest
    content: Change root Cargo.toml reqwest pin to 0.13 with json, query, rustls, stream and the invariant comment
    status: pending
  - id: compile
    content: cargo check --workspace --all-features and fix any reqwest 0.13 API fallout
    status: pending
  - id: comments
    content: Update stale 0.12 / rustls-tls / webpki-roots comments in gateway-local, workshop shell manifest, deny.toml
    status: pending
  - id: hakari
    content: cargo hakari generate, manage-deps, verify; confirm ring left workspace-hack
    status: pending
  - id: invariant
    content: "cargo tree checks: no ring in gateway, workshop, promptforge-api-runtime; single reqwest version"
    status: pending
  - id: ci
    content: Add the ring guard step to the supply-chain CI job, failing on cargo tree error as well as on a ring hit
    status: pending
  - id: docs
    content: "Update vibe/dependency-surface.md: resolve Finding 1, refresh measured counts and header commit"
    status: pending
  - id: gates
    content: Run clippy, headless gateway check, nextest, deny, build-xtask, fmt for the rustls slice; commit
    status: pending
  - id: toml-bump
    content: "toml slice: move the workspace toml pin from 0.8 to 1; fix any API fallout in gateway-config, gateway app, workshop-support, build-xtask"
    status: pending
  - id: toml-verify
    content: "toml slice: check shadow.rs round-trip and rendered-TOML fixtures; cargo tree confirms toml 0.8, toml_edit 0.19, winnow 0.5 are gone; hakari regenerate"
    status: pending
  - id: toml-gates
    content: "toml slice: rerun the gates, update dependency-surface.md, commit"
    status: pending
isProject: false
---

# Single rustls backend in the gateway

All paths below are relative to the PromptForge repository root (`promptforge/` in the workspace). Measurements were taken at commit `92620c7e` on 2026-09-18 with cargo 1.98.0.

<product-contract>

## Product Requirements

The PromptForge gateway is the credential boundary: the only process that holds vendor API keys and the only one with an edge to an LLM backend. Today it compiles and links two rustls cryptography backends, `ring` and `aws-lc-rs`, because the workspace pins `reqwest 0.12` (which selects `ring`) while Tauri and `hf-hub` pull `reqwest 0.13` (which selects `aws-lc-rs`) into the same unified build through `workspace-hack`. Two C and assembly crypto libraries in the process that holds secrets means two advisory streams and a `CryptoProvider` rustls cannot choose on its own. The fix is to move the workspace to `reqwest 0.13`, which has no `ring` option, and to guard the single-backend property in CI. A small companion change unifies the `toml` pin on the 1.x line the tree already resolves.

- Problem and users: the gateway binary links both `ring` and `aws-lc-rs`. Evidence: `cargo tree -p gateway -e normal -i rustls@0.23.45 -f '{p} [{f}]' --depth 0` prints `rustls v0.23.45 [aws-lc-rs,aws_lc_rs,ring,std,tls12]`. The gateway has zero call sites on `reqwest 0.13`; it arrives only through `crates/workspace-hack/Cargo.toml`, pulled by `tauri`, `tauri-plugin-updater`, and `hf-hub`. Users are gateway operators and anyone who audits its supply chain.
- Goals:
  - Exactly one rustls crypto backend, `aws-lc-rs`, in the gateway, the Workshop binary, and the `promptforge-api-runtime` library closure.
  - A CI check that fails when `ring` re-enters the gateway's shipped closure.
  - One `reqwest` version in the tree.
  - One workspace-pinned `toml` version: the `toml 0.8` line leaves every shipping platform, and with it the `toml_edit 0.19` and `winnow 0.5` tail on Windows and macOS (5 exclusive crates on Windows, 1 on Linux). On Linux, `toml_edit 0.19` and `winnow 0.5` remain reachable through Tauri's GTK stack (`gtk3-macros` -> `proc-macro-crate 1` -> `toml_edit 0.19`, and `glib-macros` -> `proc-macro-crate 2` -> `toml_edit 0.20` for `winnow 0.5`), transitive pins the workspace does not own and out of scope.
- Non-goals:
  - Changing which HTTP client the executor or gateway uses, or where vendor credentials live.
  - Reducing the `workspace-hack` unified-set floor (381 packages under every member); see Deferred.
  - Replacing `turso`; see Deferred.
  - Adding any structural check beyond the one CI step the user approved.
- Success criteria:
  - `cargo tree -p gateway -e normal -i ring` prints nothing; same for `-p workshop` and `-p promptforge-api-runtime`.
  - `cargo tree -p gateway -e normal -i rustls -f '{p} [{f}]' --depth 0` lists `aws-lc-rs` and not `ring`.
  - `cargo tree -p gateway -e normal -i reqwest` shows one version.
  - `cargo tree --workspace -e normal --target all -i toml@0.8` prints nothing.
  - Every existing verification gate passes; `cargo hakari verify` is clean; `vibe/dependency-surface.md` records the resolution with fresh measurements.
- Constraints:
  - Rust MSRV in the workspace is 1.89 or later (`crates/promptforge-api-runtime/README.md`); `reqwest 0.13.4` requires 1.85, so no toolchain change.
  - The workspace forbids `unsafe_code` and denies `unwrap_used`, `expect_used`, and pedantic clippy (`Cargo.toml`, `[workspace.lints]`); any compile-fix code must satisfy those lints.
  - Repository policy admits a new structural check only with explicit user approval; the user approved exactly one: a CI step failing when `ring` is in the gateway's normal-edge closure.
  - A Cargo feature gates a real constraint, never product shape; `default-features = false` on `reqwest` stays.
  - Runtime and serve paths never compile native dependencies; `aws-lc-rs` builds C at `cargo build` time, which is already the case today (it is in the shipped closure at `92620c7e`), so this constraint is unchanged by the switch.
- Open questions: None

## Functional Specification

No user-visible feature changes. Two behaviors of the gateway's outbound TLS change: the cryptography backend collapses to `aws-lc-rs` alone, and the set of trusted root certificates moves from the bundled Mozilla list (`webpki-roots`, via `reqwest 0.12`'s `rustls-tls`) to the operating system trust store (`rustls-platform-verifier`, `reqwest 0.13`'s default under its `rustls` feature). Everything else (request shapes, timeouts, redirect policies, streaming, the blocking client used for artifact downloads) is preserved.

- Actors and workflows:
  - Gateway operator: runs the gateway unchanged; TLS to upstreams now validates against the OS store, so a corporate proxy root installed in the OS store is trusted, as it already is for the Workshop's Tauri updater and the picker's `hf-hub` fetch.
  - Developer: `cargo build` behaves as before; CI gains one supply-chain step.
- Inputs and outputs: unchanged. Config files, the `/v1` and `/admin` routes, and the discovery file are untouched.
- States and validation: unchanged.
- Errors and recovery: unchanged error taxonomy. A TLS failure against a host the OS does not trust still surfaces as the existing upstream-transport error class (`crates/gateway/protocol/src/error.rs`, `upstream_transport`).
- Security and privacy behavior:
  - One crypto backend in the credential-boundary process, so one advisory stream and an unambiguous `CryptoProvider`.
  - Root trust follows the OS store. Self-signed and otherwise untrusted upstreams fail as before; hosts trusted only by the Mozilla bundle but not by the OS store would newly fail, and hosts trusted only by the OS store would newly succeed.
- Acceptance criteria: the success criteria above, plus the full gate list in Testing Plan.

</product-contract>
<implementation-contract>

## Technical Design

Three manifest pins move (`reqwest`, `toml`, and `tauri-plugin-updater`'s default features), the hakari unification crate is regenerated, stale comments are corrected, one CI step is added, and the dependency-surface record is refreshed. No Rust module boundaries or public APIs change; the compile-fix pass is expected to touch little or nothing because the `reqwest` API surface in use survived 0.13 unchanged.

- Architecture: unchanged. The gateway remains the only process with an edge to an LLM backend; the executor's `GatewayClient` (`crates/promptforge/model-client`) still speaks to the gateway over loopback.
- Modules and interfaces: no interface changes. The `reqwest` surface in use, verified by search across `crates/`, is `Client::new`, `Client::builder()` with `timeout`, `connect_timeout`, `redirect`, `https_only`, and `dns_resolver`; `blocking::Client`, `blocking::get`, and `blocking::Response`; `redirect::Policy`; `dns::{Resolve, Resolving, Addrs, Name}`; `header`; `StatusCode`; `Method`; `Response::bytes_stream`, `error_for_status`; and `RequestBuilder::query`. All exist in 0.13. Nothing uses `use_rustls_tls`, `form`, `text_with_charset`, `add_root_certificate`, or `trust-dns`.
- File and public API changes:
  - `Cargo.toml` (root, `[workspace.dependencies]`): `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }` becomes `version = "0.13"` with `features = ["json", "query", "rustls", "stream"]`, `default-features = false` retained. `rustls-tls` was renamed `rustls` in 0.13; `query` became opt-in in 0.13 and is used in `crates/gateway/app/src/hf.rs`, `crates/gateway/cloud-providers/src/providers/*.rs`, and `crates/gateway/web-search/src/brave.rs`. Add a manifest comment stating the invariant: one rustls backend, `aws-lc-rs`, the one Tauri and `hf-hub` already select, so a bump cannot re-enable `ring`; roots from the OS store through the platform verifier.
  - `Cargo.toml` (root): `toml = "0.8"` becomes `toml = "1"`. `tauri-utils` already resolves `toml 1.1`, so this removes the second line rather than adding one.
  - Per-crate `reqwest` feature additions are unchanged: `stream` in `crates/gateway/app/Cargo.toml`; `blocking` in `crates/gateway/local/Cargo.toml` and `crates/gateway/config/Cargo.toml`; `gzip`, `brotli` in `crates/promptforge/webfetch/Cargo.toml`.
  - `Cargo.toml` (root): `tauri-plugin-updater` gains `default-features = false, features = ["system-proxy", "zip"]`. Its default `rustls-tls` feature depends on `rustls` with `features = ["ring"]` and installs `ring` as the process-global `CryptoProvider`; through `workspace-hack` that feature alone puts `ring` back into every member, the gateway included, after the `reqwest` pin moves. The plugin's `reqwest` calls keep TLS because cargo unifies `reqwest` features per binary and the workspace pin enables `rustls` (`aws-lc-rs`). `system-proxy` and `zip` are the plugin's other defaults, retained.
  - `crates/workspace-hack/Cargo.toml`: regenerated by `cargo hakari generate` and `cargo hakari manage-deps` (configuration in `.config/hakari.toml`). Expected diff: the `reqwest 0.12` line disappears; the 0.13 line gains `blocking`, `brotli`, `gzip`; the per-target `rustls`, `rustls-webpki`, `tokio-rustls`, and `hyper-rustls` lines lose `ring`; the per-target `toml = { version = "0.8" }` lines disappear.
  - `.github/workflows/ci.yml`, `supply-chain` job, after the `cargo audit` step: one step that runs `cargo tree --locked -p gateway -e normal -i ring --depth 0` (`--locked` so a stale `Cargo.lock` fails the guard instead of being re-resolved silently), fails if the command errors, and fails if the output is non-empty. The job already feeds `ci-green`, so `needs:` is unchanged. Sketch:

    ```yaml
    - name: One rustls crypto backend in the gateway
      shell: bash
      run: |
        set -euo pipefail
        out="$(cargo tree --locked -p gateway -e normal -i ring --depth 0 2>&1 || { echo "cargo tree failed: $out"; exit 1; })"
        if printf '%s' "$out" | grep -v '^warning: nothing to print' | grep -q 'ring'; then
          echo "ring is linked into the gateway; aws-lc-rs is the only permitted rustls backend"
          printf '%s\n' "$out"
          exit 1
        fi
    ```

    The exact shell may be adjusted; the two required properties are that a `cargo tree` failure fails the step and that only a successful, ring-free result passes.
  - Comment corrections, no behavior change: `crates/gateway/local/src/artifacts.rs` (near line 67) and `crates/gateway/local/src/artifacts/download.rs` (near line 264) say "pinned blocking reqwest client (0.12)"; make version-neutral. `crates/workshop/shell/Cargo.toml` (near line 24) says "(json, rustls-tls)"; becomes "(json, rustls)". `deny.toml` says `webpki-roots` arrives via `reqwest rustls-tls`; reword after confirming the post-switch puller with `cargo tree -i webpki-roots` (the platform verifier may still use it on Linux).
  - `vibe/dependency-surface.md`: move Finding 1 from "Open findings" to "What was replaced, and why" with the before and after `rustls` feature lines as evidence; remove the `reqwest 0.12/0.13` entry from the duplicate-versions list; remove the `toml 0.8` row from the ranked table and from the smaller candidates; refresh the shipped-shape counts and the header commit and date using the commands in that file's "Reproducing the measurements" section.
- Data, persistence, failure, security, and privacy constraints:
  - `toml 1.x` changed `to_string_pretty` layout for arrays and tables. Rendered-TOML comparisons exist in `crates/gateway/config/src/shadow.rs` (near line 344) and its tests in `crates/gateway/config/src/shadow-tests.rs`, `crates/gateway/config/src/profile.rs` (near line 169), and fixtures under `crates/gateway/config/src/config/tests/`. A formatting-only fixture diff is updated; a round-trip that loses or reorders data is a stop-and-re-plan condition. The `toml` API in use (`Value`, `map::Map`, `Table`, `Spanned`, `de::Error`, `from_str`, `to_string`, `to_string_pretty`) exists in 1.x; consumers are `crates/gateway/config/src/config/imp.rs`, `shadow.rs`, `profile.rs`, `api_error.rs`, `crates/gateway/app/src/config_write.rs`, `crates/workshop/support/src/config.rs`, and `crates/build-xtask/src/{product,tidy,new_crate}.rs`.
  - `Cargo.lock` changes and is committed; `--locked` gates run against the updated lock.
  - No persisted data, protocol, or lifecycle behavior changes.

</implementation-contract>
<verification-contract>

## Testing Plan

Verification is the workspace's existing gate list plus the four `cargo tree` invariant checks. No new tests are written for the rustls change because the property is a link-time fact the CI step checks directly; the toml change is verified by the existing config round-trip and fixture tests.

- Unit: existing suites unchanged. Watch `crates/gateway/config` (shadow round-trip, profile serialization, config fixtures) for `to_string_pretty` formatting diffs under `toml 1.x`.
- Integration and end-to-end: existing gateway integration suites (`crates/gateway/app/tests/it/`) and Workshop server suites run unchanged. Any test asserting a TLS failure against a self-signed or untrusted host must still fail the same way; the OS store does not trust those either.
- Regression, security, and performance:
  - `cargo tree -p gateway -e normal -i ring` prints nothing; same for `-p workshop` and `-p promptforge-api-runtime`.
  - `cargo tree -p gateway -e normal -i rustls -f '{p} [{f}]' --depth 0` lists `aws-lc-rs` and not `ring`.
  - `cargo tree -p gateway -e normal -i reqwest` shows one version.
  - `cargo tree --workspace -e normal --target all -i toml@0.8` prints nothing.
  - `cargo hakari verify` clean.
  - `cargo deny check` clean (license allowlist and duplicate-version warnings in `deny.toml`).
- Exit criteria: the workspace's verification commands, as recorded in `AGENTS.md` (Verification section), all pass:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `cargo test -p build-xtask`
  - `cargo deny check`

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Move to `reqwest 0.13` rather than keep 0.12 with `rustls-tls-no-provider` or exclude the 0.13 pullers from hakari traversal. Rationale: 0.13 has no `ring` option, so the invariant is enforced by feature resolution instead of by process-global provider installation (which the workspace forbids in library and serve paths) or by hakari configuration that must be maintained. User's words: "well fix it".
  - Trust the OS certificate store (`rustls-platform-verifier`, the 0.13 default) rather than pin the Mozilla bundle. Rationale: it is reqwest's new default, it is already what the Tauri updater and `hf-hub` do inside the same processes, and desktop users behind corporate TLS proxies get working TLS. User selected "OS trust store".
  - Add one CI step failing when `ring` is in the gateway's normal-edge closure. Rationale: a security boundary with no ordinary test equivalent, the case the repository's structural-check policy carves out. User selected "Yes, add the CI step".
  - Include the `toml 0.8` to `1` unification in this plan. Rationale: it is the workspace's own pin, `tauri-utils` already resolves `toml 1.1`, and the same gates verify it. User selected "rustls fix plus the toml 0.8 to 1.x bump as a second commit".
  - Each of the two changes lands in its own commit. User's words as above.
  - Keep `default-features = false` on `reqwest`. Rationale: today's set already omits `charset`, `http2`, and the system-proxy default; adding them would be product-shape change unrelated to the fix.
  - Disable `tauri-plugin-updater`'s default features, keeping `system-proxy` and `zip`. Rationale: discovered during implementation that the plugin's default `rustls-tls` feature was a second, independent `ring` enabler, so the `reqwest` pin alone did not satisfy the single-backend goal for the Workshop binary or, through `workspace-hack`, the gateway. Disabling it is one manifest line, reversible, and the plugin retains TLS through the unified `reqwest 0.13` `rustls` feature. Falsifier: a Workshop update check over https fails with a reqwest "no TLS backend" error.
- Rejected alternatives:
  - Keep `reqwest 0.12` with `rustls-tls-no-provider` and install a `CryptoProvider` at boot. Reason: installs process-global state; `promptforge-api-runtime` is a library a host embeds, so the host would inherit the obligation. Revisit if 0.13 proves unusable for a reason not found in the API survey.
  - Keep 0.12 and exclude Tauri and `hf-hub` from hakari traversal so 0.13 stops reaching the gateway. Reason: fixes the symptom by configuration that must be maintained, and leaves two backends in the Workshop binary. Revisit under the deferred hakari-floor trial, for build-size reasons rather than for this invariant.
  - Pin the Mozilla bundle with `webpki-roots` and `tls_certs_only` on every client builder. Reason: preserves today's exact trust list at the cost of a call on each of the roughly ten client construction sites and a divergence from the Tauri and `hf-hub` clients in the same process. Revisit if an operator reports an upstream trusted by the bundle but not by an OS store.
  - Record the invariant only in `vibe/dependency-surface.md` with no CI step. Reason: the user chose enforcement.
- Assumptions, risks, and notes:
  - `aws-lc-rs 1.18.1` is already in the gateway's shipped closure at `92620c7e` (`cargo tree -p gateway -e normal -i aws-lc-rs`), so its native build requirements (a C compiler; NASM or the prebuilt-NASM path on Windows MSVC) are already met by every current build. The switch adds no new toolchain requirement.
  - `cargo-hakari` must be installed locally (`cargo install cargo-hakari`) to regenerate `crates/workspace-hack/Cargo.toml`. `.config/hakari.toml` names the five shipping target triples; run `cargo hakari generate` after each manifest pin change.
  - Whether `webpki-roots` remains in the tree after the switch depends on `rustls-platform-verifier`'s Linux path; confirm with `cargo tree -i webpki-roots` before rewording the `deny.toml` comment. The CDLA-Permissive-2.0 allow entry stays either way.
  - The supply-chain CI job runs on `ubuntu-latest` and installs `cargo-deny` and `cargo-audit` through `taiki-e/install-action`; `cargo tree` needs a Rust toolchain, which `rust-toolchain.toml` provisions through rustup on that runner.
  - `toml 1.x` formatting change is the one place this plan may produce test diffs; the stop condition is a data-changing round trip, not a layout change.
  - The gateway user guide (`guide/promptforge-gateway-guide.md`) contains no statement about root certificates, so no guide edit is required; adding one is optional and outside this plan.
  - The `dependency-surface.md` refresh re-runs the measurements; the exclusive-cost ranking script is not in the repository by design and is re-derived from that file's method section.

### Deferred and Out of Scope

- Deferred: the `workspace-hack` floor (381 packages under every member, Finding 2 in `vibe/dependency-surface.md`). A measured `[traversal-excludes]` trial in `.config/hakari.toml` naming the Workshop-only pullers (`tauri` and its plugins, `hf-hub`); accept only if the gateway's shipped package count drops by 40 or more and the `cargo build -p workshop` rebuild cost is tolerable. Revisit after both slices in this plan land, since they change the unified set.
- Deferred: `turso` at 59 exclusive crates (Finding 3). Revisit when the product decides whether the sync and replication that justify `turso` over `rusqlite` will be used.
- Out of scope: `readabilityrs` and `htmd` (17 crates, 0.x, watch), `nvml-wrapper` (3 crates, keep), transitive duplicate versions not pinned by the workspace.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (default-members is only `crates/gateway/app`; the desktop app is an explicit `cargo build --locked -p workshop`, and the headless shape is `cargo build --locked -p gateway --no-default-features`). Rust stable channel via `rust-toolchain.toml`, edition 2024. Windows links with `rust-lld` and `+crt-static` per `.cargo/config.toml`. Cargo aliases: `cargo xtask` (build-xtask), `cargo workshop` (build-workshop). The two esbuild UIs need `npm ci --prefix crates/workshop/server/ui` and `npm ci --prefix crates/gateway/config-ui/ui` before workspace-wide clippy, nextest, or doc runs.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <name-filter>` for unit and integration tests; `cargo test --locked -p <crate> --test it <name-filter>` for one integration target (for example `cargo test -p gateway-stt --test it architecture`). Doctests only via `cargo test --doc -p <crate>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`; workshop family together as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` (plus `cargo nextest run --locked -p workshop-server --features headless`); TypeScript UIs as `npm test` inside `crates/workshop/server/ui` or `crates/gateway/config-ui/ui` (runs `node --test`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the workshop set above, then `cargo test -p build-xtask` (boundary and structural harness). Nextest config in `.config/nextest.toml`: 60s slow-timeout, terminate after 3, 250ms leak timeout, `heavy` test group (max 8 threads, 4 per test) for promptforge-tool-picker, gateway-stt-backend-whisper, gateway-stt.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, then the feature-shape gate `cargo check -p gateway --no-default-features`. Never run a standalone `cargo check --workspace`. Supply-chain gates in CI and the pre-push hook: `cargo deny check` (`deny.toml`), `cargo audit`. TypeScript: `npm run typecheck` (`tsc --noEmit`) in each UI directory.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; this is the `.githooks/pre-commit` hook).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; user guide via `mdbook build guide`.
- Test placement and naming conventions: unit tests live inline as `#[cfg(test)] mod tests` (about 300 occurrences) or, when large, as a `src/<module>/tests/` subdirectory with `mod.rs` plus topic files (for example `crates/promptforge-api-runtime/src/execute/tests/`, `crates/gateway/config/src/config/tests/`). Integration tests use a single target `tests/it/main.rs` with topic modules beside it (`tests/it/boot.rs`, `tests/it/chat.rs`), shared helpers in `tests/common/mod.rs`, data in `tests/fixtures/` (JSON, audio) or `tests/prompts/` (Markdown pipelines for the runtime). Test names are long snake_case sentences (`a_direct_launch_recovers_the_lease_from_a_terminated_owner`). Benches sit in `benches/` for promptforge-lua and promptforge-api-runtime. Clippy allows `unwrap`/`expect` in tests only (`clippy.toml`). TypeScript tests are `*.test.mjs` beside sources or under `test/`; Node tool scripts in `tools/` carry sibling `*.test.mjs` files.
- Directory map: `Cargo.toml` is the workspace root (resolver 3, explicit member list, `workspace-hack` via cargo-hakari in `.config/hakari.toml`, workspace-wide lints: `unsafe_code = "forbid"`, `missing_docs`/`unreachable_pub` warn, clippy `all` + `pedantic` deny, `unwrap_used`/`expect_used` deny, `doc_markdown` allow). `crates/` root holds the public layer and meta tooling: `promptforge-api-runtime`, `promptforge-api-types`, `gateway-api`, `gateway-api-discovery`, `shared-loopback`, `shared-progress`, `shared-vfs`, `shared-ui` (TypeScript+CSS package, not a Rust crate), `workspace-hack`, and `build-*` crates (`build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`). `crates/promptforge/` is the manifestless private container for `lua`, `parser`, `store`, `vfs`, `model-client`, `tool-picker`, `web`, `webfetch`, `web-search`. `crates/gateway/` holds `app` (the `gateway` binary), `cloud-providers`, `config`, `config-ui` (with an esbuild UI under `ui/`), `local`, `logging`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api`, `engine`, `backend-whisper`, `whisper-ffi`). `crates/workshop/` holds `shell` (package `workshop`, Tauri), `server` (with its SPA under `ui/`), `server-api`, `gateway`, `menu`, `protocol`, `registry`, `sessions`, `status`, `support`, `user-state`, `workspace`. Other top-level directories: `guide/` (mdbook user guide, `book.toml`, `src/`), `prompts/` (example `.md` pipelines), `local/` (gitignored dev config and fixtures), `tools/` (Node staging and TTS scripts), `vibe/` (plans by date, `archdoc.md`, `ACTIVE`), `.github/workflows/` (ci, nightly, releases, stt-miri, guide), `.githooks/` (pre-commit fmt, pre-push check/clippy/deny; `core.hooksPath` is not set locally), `deny.toml`, `dist-workspace.toml` (cargo-dist), `gateway.local.example.toml`.
- Component boundaries: three products, one-way dependencies. `shared-*` crates depend on no product crate. PromptForge's only public surface is `promptforge-api-runtime` and `promptforge-api-types`; `crates/promptforge/*` is private to the family and only `promptforge-api-runtime` may depend into it. Gateway's public surface is `gateway-api` and `gateway-api-discovery`; `crates/gateway/*` is private, and `crates/gateway/stt/` exposes only `gateway-stt` to the rest of the family. Gateway crates must not depend on promptforge or workshop crates; PromptForge crates must not depend on gateway or workshop crates; workshop crates must not depend on gateway crates except the public pair. Container crates may depend only on `crates/` root crates and their own siblings. The `workshop` shell depends on `workshop-server-api`, never `workshop-server`. Per `archdoc.md`: executor depends on gateway, store, Lua VM boundary, shared substrate; gateway depends on shared substrate only; store (`vfs.store(&access)`) sits over the VFS layer (`shared-vfs` + `promptforge-vfs`); the Lua VM boundary depends on gateway, store, shared substrate. `build-*` crates are exempt from container privacy. `cargo test -p build-xtask` enforces the product matrix, tier graph, lint inheritance, and the 500-line ceiling.
- Conventions summary: reuse existing facilities before adding machinery; no structural checks (parsers, allowlists, counts, ceilings) without explicit user approval; behavior changes ship with tests in the same change. Cargo features gate real constraints only (toolchain, heavy native build), never product shape; runtime and serve paths never compile native code, exit the process, or install process-global state. `unsafe_code` is forbidden workspace-wide; the STT crates opt in through an `unsafe-code` feature checked separately in CI, and every unsafe block documents its invariants. Comments explain non-obvious constraints and cite upstream issue URLs for workarounds; dependency pins in `Cargo.toml` carry a comment stating why (version unification, default-features off rationale). `reqwest` is declared once at the workspace level with `default-features = false` and `rustls-tls`. Long-running work reports through `shared-progress`. Error messages are concise, self-contained, and name required versus actual. Source directories are flat: a subdirectory needs at least three files, otherwise use `foo-bar.rs` siblings with `#[path]`; no file over 500 lines. Every workshop crate `lib.rs` and SPA concern `index.ts` opens with a `## Invariants` doc block. SPA rules: CSS beside its TypeScript, `--ws-*` tokens only, no `localStorage` (state goes through `ui-storage` to `workshop-user-state` or the `.pfwork` workspace file). Plans live in `vibe/YYYY-MM-DD-N-slug.md`; `vibe/individual-commits.md` and `vibe/dependency-surface.md` are standing references.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Single rustls backend through reqwest 0.13 [completed]

- Component: `none`
- Objective: the `gateway`, `workshop`, and `promptforge-api-runtime` closures link exactly one rustls crypto backend, `aws-lc-rs`, and CI fails if `ring` returns.
- Manifest change: in the root `Cargo.toml` `[workspace.dependencies]`, change `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }` to `version = "0.13"` with `features = ["json", "query", "rustls", "stream"]`, keeping `default-features = false`. Add a comment above the pin stating the invariant: one rustls backend, `aws-lc-rs`, the one Tauri and `hf-hub` already select, so a bump cannot re-enable `ring`; roots come from the OS store through the platform verifier. Leave the per-crate feature additions (`stream` in `crates/gateway/app/Cargo.toml`; `blocking` in `crates/gateway/local/Cargo.toml` and `crates/gateway/config/Cargo.toml`; `gzip`, `brotli` in `crates/promptforge/webfetch/Cargo.toml`) unchanged.
- Updater pin: in the root `Cargo.toml`, change `tauri-plugin-updater = "2.11.0"` to `{ version = "2.11.0", default-features = false, features = ["system-proxy", "zip"] }`. The plugin's default `rustls-tls` feature depends on `rustls` with `features = ["ring"]`, so with it enabled the `reqwest` pin alone leaves `ring` in every closure through `workspace-hack`. The plugin's `reqwest` calls keep TLS because cargo unifies `reqwest` features per binary and the workspace pin enables `rustls` (`aws-lc-rs`); Step 1 relies on that unification, and the `workshop` invariant check below is what proves it held. Add a manifest comment stating this. `system-proxy` and `zip` are the plugin's other defaults, retained.
- Compile fix: run `cargo check --workspace --all-features` once as the pin-change probe (this is the only permitted use; the standing rule against a standalone `cargo check --workspace` still applies afterward). Fix any `reqwest 0.13` fallout at the call sites in `crates/gateway/app/src/hf.rs`, `crates/gateway/cloud-providers/src/providers/*.rs`, `crates/gateway/web-search/src/brave.rs`, `crates/gateway/local/src/artifacts/download.rs`, `crates/gateway/config`, and `crates/promptforge/webfetch` under the workspace lint set (`unsafe_code` forbid, `unwrap_used` and `expect_used` deny, pedantic clippy). The surveyed API surface (`Client::builder` with `timeout`, `connect_timeout`, `redirect`, `https_only`, `dns_resolver`; `blocking::{Client, get, Response}`; `redirect::Policy`; `dns::{Resolve, Resolving, Addrs, Name}`; `header`; `StatusCode`; `Method`; `Response::bytes_stream`, `error_for_status`; `RequestBuilder::query`) exists in 0.13, so little or no code change is expected.
- Hakari: run `cargo hakari generate` then `cargo hakari manage-deps` (`.config/hakari.toml`). Confirm in the `crates/workspace-hack/Cargo.toml` diff that the `reqwest 0.12` line is gone, the 0.13 line gained `blocking`, `brotli`, `gzip`, and every per-target `rustls`, `rustls-webpki`, `tokio-rustls`, and `hyper-rustls` line lost `ring`. `cargo hakari verify` must be clean. Commit the updated `Cargo.lock`.
- Invariant checks (all after hakari): `cargo tree -p gateway -e normal -i ring` prints nothing, and likewise for `-p workshop` and `-p promptforge-api-runtime`; `cargo tree -p gateway -e normal -i rustls -f '{p} [{f}]' --depth 0` lists `aws-lc-rs` and not `ring`; `cargo tree -p gateway -e normal -i reqwest` shows one version.
- Comment corrections (no behavior change): make the "pinned blocking reqwest client (0.12)" comments version-neutral in `crates/gateway/local/src/artifacts.rs` (near line 67) and `crates/gateway/local/src/artifacts/download.rs` (near line 264); change "(json, rustls-tls)" to "(json, rustls)" in `crates/workshop/shell/Cargo.toml` (near line 24); in `deny.toml`, run `cargo tree -i webpki-roots` first and reword the `webpki-roots` comment to name the actual post-switch puller (or state that it left the tree), keeping the CDLA-Permissive-2.0 allow entry either way.
- CI guard: in `.github/workflows/ci.yml`, `supply-chain` job, add one step after the `cargo audit` step named `One rustls crypto backend in the gateway` that runs `cargo tree --locked -p gateway -e normal -i ring --depth 0` under `set -euo pipefail`. `--locked` is required so a stale `Cargo.lock` fails the guard rather than being re-resolved silently. It must fail when `cargo tree` itself errors and fail when the output (after discarding the `warning: nothing to print` line) contains `ring`; only a successful, ring-free result passes. Use the sketch in the Technical Design; adjust the shell as needed while keeping both failure properties. `needs:` is unchanged because the job already feeds `ci-green`.
- Dependency-surface record: in `vibe/dependency-surface.md`, move Finding 1 from "Open findings" into "What was replaced, and why" with the before and after `rustls` feature lines (`[aws-lc-rs,aws_lc_rs,ring,std,tls12]` versus the post-switch line) as evidence; remove the `reqwest 0.12/0.13` entry from the duplicate-versions list; refresh the shipped-shape counts and the header commit and date using that file's "Reproducing the measurements" section so the record matches this commit's lock. Leave the `toml 0.8` rows in place; Step 2 removes them.
- Gates before commit: `cargo fmt --all --check`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; `cargo check -p gateway --no-default-features`; `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; `cargo test -p build-xtask`; `cargo deny check`. Any existing test that asserts a TLS failure against an untrusted host must still fail the same way.
- Commit: one commit with a `build(deps):` message covering the pin, lock, hakari output, comment corrections, CI step, and dependency-surface update.
- Stop condition: two consecutive failures with the same signature on any one item above.

</step-1>

<step-2>

### Step 2: Unify toml on 1.x [completed]

- Component: `none`
- Objective: one workspace-pinned `toml` version; the `toml 0.8` line is gone from every shipping platform, and its `toml_edit 0.19` and `winnow 0.5` tail is gone wherever nothing else pins it (Windows and macOS; on Linux the pair remains through `gtk3-macros` -> `proc-macro-crate 1` and `glib-macros` -> `proc-macro-crate 2`, transitive pins the workspace does not own).
- Manifest change: in the root `Cargo.toml` `[workspace.dependencies]`, change `toml = "0.8"` to `toml = "1"`. `tauri-utils` already resolves `toml 1.1`, so this removes a line rather than adding one; keep or add the unification rationale comment on the pin per the repository's dependency-pin convention.
- Compile fix: run `cargo check --workspace --all-features` once as the pin-change probe. Fix any `toml 1.x` fallout in the consumers: `crates/gateway/config/src/config/imp.rs`, `crates/gateway/config/src/shadow.rs`, `crates/gateway/config/src/profile.rs`, `crates/gateway/config/src/api_error.rs`, `crates/gateway/app/src/config_write.rs`, `crates/workshop/support/src/config.rs`, and `crates/build-xtask/src/{product,tidy,new_crate}.rs`. The API in use (`Value`, `map::Map`, `Table`, `Spanned`, `de::Error`, `from_str`, `to_string`, `to_string_pretty`) exists in 1.x.
- Fixture review: `to_string_pretty` layout for arrays and tables changed in 1.x. Run `cargo nextest run --locked -p gateway-config --all-features` and inspect diffs in `crates/gateway/config/src/shadow.rs` (near line 344), `crates/gateway/config/src/shadow-tests.rs`, `crates/gateway/config/src/profile.rs` (near line 169), and the fixtures under `crates/gateway/config/src/config/tests/`. Update fixtures only where the difference is formatting; a round trip that loses or reorders data is the stop condition below, not a fixture update.
- Hakari: `cargo hakari generate`, `cargo hakari manage-deps`, `cargo hakari verify`. Confirm the per-target `toml = { version = "0.8" }` lines left `crates/workspace-hack/Cargo.toml`. Commit the updated `Cargo.lock`.
- Invariant check: `cargo tree --workspace -e normal --target all -i toml@0.8` prints nothing. `cargo tree --workspace -e normal --target x86_64-pc-windows-msvc -i toml_edit@0.19` and the same for `winnow@0.5` print nothing. Under `--target all` every remaining path to `toml_edit 0.19` or `winnow 0.5` passes through `proc-macro-crate 1` (`gtk3-macros`) or `proc-macro-crate 2` (`glib-macros`), both inside Tauri's Linux GTK stack, and `vibe/dependency-surface.md` records that residue and both owners.
- Dependency-surface record: in `vibe/dependency-surface.md`, remove the `toml 0.8` row from the ranked exclusive-cost table and from the smaller candidates, remove its duplicate-versions entry, and refresh the shipped-shape counts and the header commit and date so they reflect the final lock with both pin changes landed.
- Gates before commit: the same full list as Step 1.
- Commit: one commit with a `build(deps):` message covering the pin, lock, hakari output, fixture diffs, and dependency-surface update.
- Stop conditions: two consecutive failures with the same signature on any one item above, or a `toml` round trip that loses or reorders data (stop and re-plan; do not adjust the fixture to match).

</step-2>

</execution-plan>
