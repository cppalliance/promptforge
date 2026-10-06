---
name: Whisper CUDA backend debt removal
overview: >-
  Remove the two debts the whisper-cuda-backend branch added, as the debt collector found and
  challenged them against cppalliance master: under the default auto on Windows, a CUDA device
  identifier the probe cannot match still selects the CUDA build, whose CPU fallback crashes the
  gateway at a graceful stop; and the speech retirement logs a false "did not retire" warning when
  the command worker has already spent the shared deadline.
todos:
  - id: export-vibe-plan
    content: Run /export-vibe-plan before any other step
    status: pending
isProject: false
---

# Whisper CUDA backend debt removal

## Export this plan

Before the first implementation change, run `/export-vibe-plan`. It writes this plan under each touched repository's top-level `vibe/` directory as `YYYY-MM-DD-N-words.md`.

## Product Requirements

- Problem and users:
  - The users are gateway operators on Windows x86-64, and anyone, person or model, who reads the gateway's log after a stop.
  - The whisper CUDA backend work, branch `whisper-cuda-backend` at `356e3dc7` (pull request #91 against cppalliance/promptforge `master` at `b845508c`), left two defects that its own review of its 19 commits found:
    - DB-1: under the default `auto` on Windows, a `CUDA_VISIBLE_DEVICES` whose first entry is a `GPU-` or `MIG-` identifier naming no GPU still selects the published CUDA build. CUDA then sees no device, the build decodes on the CPU with OpenMP, and the graceful-stop retirement that unloads it crashes the gateway with `0xC0000005`.
    - DB-2: when a command ignores its cancellation token for the whole 5 s join, the speech retirement logs `speech did not retire within 5s; abandoning it` without having run. It does so even when nothing is published, so the warning reports a failure that never happened.
- Goals:
  - Under the default `auto`, no Windows machine reaches the stop crash through a device identifier the probe cannot match.
  - The retirement's warnings state what happened, by speech's state and by whether any time remained.
- Non-goals:
  - Rebuilding the Windows CUDA archive without OpenMP.
  - Matching `GPU-` or `MIG-` identifiers against the GPUs' UUIDs.
  - The explicit-`cuda` stop crash on a Windows machine whose GPU CUDA cannot use, an accepted residual the docs already state.
  - Pre-existing debt the same review exposed, listed under the Debt Inventory.
  - Changing the shared shutdown deadline or its documented bounds.
- Success criteria:
  - Under `auto` on Windows x86-64, a `CUDA_VISIBLE_DEVICES` whose first entry is not a device index selects `windows-x86_64`, and a gateway so configured on a Windows NVIDIA machine exits 0 at every graceful stop after transcriptions.
  - Linux selection and every explicit setting are unchanged.
  - A speech-less gateway whose command worker exhausts the join deadline logs no speech warning.
  - With idle published speech in that case, it logs one warning saying the retirement was not awaited because the command worker used the deadline.
  - A retirement that runs out while draining still logs `speech did not retire within 5s; abandoning it`.
  - The guide, the README, the example config, and the whisper CUDA backend plan's statements match the new behavior, and every exit criterion passes.
- Constraints:
  - Every published whisper archive, the Windows CUDA one included, is pinned and never changes, and the Windows CUDA archive keeps OpenMP.
  - Selection is the only guard against the runtime's process-ending failures, and its inputs stay arguments, so tests run on any machine.
  - Error and status messages are written for model consumption: concise, factual, self-contained, and naming what is unmet against what was required.
  - The shared shutdown deadline and its documented bounds stay as they are.
  - The work lands on `whisper-cuda-backend`, in pull request #91, with no push to cppalliance/promptforge.
- Open questions:
  - None.

## Functional Specification

- Actors and workflows:
  - An operator leaves `[stt] whisper_backend` at `auto` on a Windows NVIDIA machine whose environment sets `CUDA_VISIBLE_DEVICES`, and starts the gateway.
  - An operator stops the gateway gracefully: `POST /shutdown`, Ctrl-C, the tray, or the desktop app.
  - A person or a model reads the stop's log lines.
- Inputs and outputs:
  - Input: the `CUDA_VISIBLE_DEVICES` value, read only under `auto` where both whisper builds exist.
  - Output: the selected whisper build, and at a graceful stop, at most one speech retirement warning.
- States and validation:
  - `CUDA_VISIBLE_DEVICES` under `auto`, by its first entry:

    | First entry | Windows x86-64 | Linux x86-64 |
    | --- | --- | --- |
    | unset | visible | visible |
    | a device index below the GPU count | visible | visible |
    | empty, `-1`, an index at or above the count, or other text | hides every GPU | hides every GPU |
    | a `GPU-` or `MIG-` identifier | hides every GPU (new) | visible (unchanged) |

  - The speech retirement at a graceful stop:

    | State when the retirement starts | Outcome |
    | --- | --- |
    | nothing published | no speech warning |
    | published, the shared deadline already spent by the command worker | the retirement still runs as a spawned task; one warning says it was not awaited |
    | published, time left, retirement finishes | no warning |
    | published, time left, retirement still draining at the deadline | `speech did not retire within 5s; abandoning it`, as today |

- Errors and recovery:
  - A Windows operator with a valid UUID-form `CUDA_VISIBLE_DEVICES` gets the CPU build under `auto`; `cuda` restores GPU decoding.
  - An explicit `cuda` with an identifier naming no GPU still decodes on the CPU and ends the gateway at a graceful stop. The docs keep stating it, with `auto` or `cpu` as the recovery.
- Security and privacy behavior:
  - None. No credential, trust boundary, or stored data changes.
- Acceptance criteria:
  - The success criteria hold, and the Testing Plan's exit criteria pass.

### Debt Inventory

- DB-1, introduced: an unmatched `GPU-` or `MIG-` identifier keeps Windows `auto` on the CUDA build, whose graceful stop then crashes.
  - Code facts:
    - `cuda_visible_devices_hides_every_gpu` in `crates/gateway/local/src/artifacts/assets.rs` treats any first entry starting with `GPU-` or `MIG-` as visible.
    - The probe in `crates/gateway/local/src/artifacts.rs` queries only `compute_cap,driver_version`, so no identifier is matched. `gpus_hidden_from_cuda_select_the_cpu_whisper_build` asserts that `GPU-8f6e2c1a` keeps `windows-x86_64-cuda`.
  - Mechanism, from `4b2ab768`: `Gateway::serve` retires speech before it returns, and the retirement unloads `whisper.dll` and its ggml DLLs under the CUDA build's live OpenMP thread team.
  - Recorded outcome: with GPUs hidden, 10 of 10 graceful stops crashed with `0xC0000005`, while the gateway built just before `4b2ab768` exited 0. The guide and the README state the outcome for "such an identifier under `auto`".
  - `fedd11c6` narrowed the `auto` exposure to identifier entries and left them open. Master selects the CUDA build for this input too, but it exits cleanly.
  - Reach: low. It needs a Windows NVIDIA machine that passes the 580 floor and the native list, with a stale, foreign, or abbreviated UUID, or any `MIG-` value, since Windows has no MIG.
  - It contradicts the whisper CUDA backend goal that `auto` never ends the gateway process.
  - Reversal cost: low.
- DB-2, introduced by `4b2ab768`: the retirement's warning reports a 5 s failure that never happened.
  - Code facts:
    - `Gateway::serve` in `crates/gateway/app/src/runner.rs` reads one deadline before the command worker's join, then runs `timeout_at(deadline, spawn_blocking(speech.shutdown))`.
    - Once the join has spent the deadline, tokio polls the fresh task once, finds it pending, and returns elapsed.
  - Recorded outcome: the end-to-end stop during the 743 MB download logged the warning with nothing published. Any command that outlives its token past the join still reaches it: an extraction under way, verifying a cached model, `whisper_init`, or a stalled llama download.
  - Consequence: the false diagnostic points readers at a speech hang. The `serve` rustdoc names only a retirement "still draining" as abandoned, and the whisper CUDA backend plan reads the warning's absence as proof that speech retired.
  - Reversal cost: low.
- Exposed pre-existing debt, not counted as added and not remediated here:
  - The Windows archives import the MSVC runtime without bundling it, the same linkage the master Windows CUDA row has.
  - The boot command's failure log drops the error's cause, so the named whisper selection errors never reach the operator. Another task tracks it.
  - SIGTERM and Ctrl-Break skip the graceful stop.
- Rejected candidates: 49.
  - 13 residual-but-acceptable: documented, deliberate trade-offs, the explicit-`cuda` crash among them.
  - 24 weak or speculative: structural leads with no demonstrated consequence.
  - 7 false.
  - 5 unrelated pre-existing.

## Technical Design

- Architecture:
  - Both fixes stay inside their owners: whisper build selection in `gateway-local`, and the graceful stop in `gateway`'s `Gateway::serve`. No dependency direction, component ownership, persisted or wire format, or public interface changes.
- Modules and interfaces:
  - DB-1: `auto` on Windows x86-64 counts a `CUDA_VISIBLE_DEVICES` whose first entry is not a decimal device index as hiding every GPU.
    - The rule lives in the pure selection, in `crates/gateway/local/src/artifacts/assets.rs`, with the platform and the variable's value as arguments. Either `cuda_visible_devices_hides_every_gpu` takes whether identifiers count as visible, or the selection derives it from the platform.
    - Linux keeps counting identifiers as visible, because only the Windows CUDA build crashes at a stop after a CPU fallback.
    - Explicit `cpu` and `cuda` still skip every machine check, and llama-server's selection is untouched.
  - DB-2: `Gateway::serve` reads whether speech has a published runtime, through the speech service's existing status, before it starts the retirement.
    - With nothing published, it logs no speech warning.
    - With speech published and the deadline already spent, it still spawns the retirement, so the runtime's shutdown bound can finish it, and it logs one warning that the retirement was not awaited because the command worker used the whole deadline.
    - A retirement that starts with time left keeps today's behavior and warning.
- File and public API changes:
  - Changed: `crates/gateway/local/src/artifacts/assets.rs` and `crates/gateway/app/src/runner.rs`, with their tests and doc comments.
  - Docs:
    - `guide/src/gateway/05-speech.md`, the `whisper_backend` row of `crates/gateway/app/README.md`, and the commented `whisper_backend` lines of `gateway.local.example.toml` state the Windows identifier rule.
    - `guide/promptforge-gateway-guide.md` is regenerated with `cargo run -p build-user-guide`.
    - `vibe/2026-09-28-2-whisper-cuda-backend.md` corrects its statements on identifiers and on a stop during a long load.
  - No public API changes. The touched functions are crate-private.
- Data, persistence, failure, security, and privacy constraints:
  - The shared deadline, `WORKER_JOIN_TIMEOUT`, and `RUNTIME_SHUTDOWN_TIMEOUT` keep their values and their documented bounds. The `serve` rustdoc and `WORKER_JOIN_TIMEOUT`'s doc state the retirement's three outcomes.
  - The warning text is a log line, not a published contract.

## Testing Plan

- Unit:
  - DB-1, in `assets.rs`:
    - `cuda_visible_devices_hides_every_gpu_by_cudas_rule`: on Windows, `GPU-8f6e2c1a`, `MIG-...`, and `GPU-` hide every GPU, while `0`, `1,0`, and `0,-1` keep one visible with two GPUs. The Linux cases are unchanged.
    - `gpus_hidden_from_cuda_select_the_cpu_whisper_build`: its Windows identifier case selects `windows-x86_64`, and its Linux identifier case still selects `linux-x86_64-cuda`.
    - `an_explicit_whisper_backend_ignores_the_probe` gains a Windows identifier case under explicit `cuda`, which keeps `windows-x86_64-cuda`.
    - The flipped Windows cases fail before the change.
  - DB-2, in `drain_tests` in `crates/gateway/app/src/runner.rs`, with tracing output captured in-process through a thread-local default subscriber, as `capture_logs` in `crates/gateway/protocol/src/upstream.rs` captures it:
    - `serve_abandons_a_worker_that_ignores_cancellation_after_the_join_bound`, a speech-less gateway past an exhausted deadline, asserts that no speech warning is logged. It fails before the change.
    - A new test parks a command past the join bound with idle published scripted speech. It asserts the one not-awaited warning, and that the scripted workers drop once the retirement runs.
    - `serve_abandons_a_speech_retirement_that_outlasts_the_join_bound`, whose retirement starts with time left and is held open while draining, asserts the existing `speech did not retire` warning.
    - `serve_bounds_the_worker_join_and_the_speech_retirement_by_one_deadline` keeps its timing assertions. Its stuck command spends the deadline, so it now takes the not-awaited path.
- Integration and end-to-end:
  - On this Windows NVIDIA machine, the scratch scripts in `vibe/scratch/run-36900875610/scripts/` run on a clone of the change, with the Windows CPU row pointed at CI run 36900875610's archive.
    - `win_gateway.py hidden` with `CUDA_VISIBLE_DEVICES=GPU-00000000` takes `b4938-windows-x86_64`, and every stop exits 0.
    - `win_gateway.py boots` with the variable unset still takes the CUDA build on the RTX 3090s.
- Regression, security, and performance:
  - `cargo test --locked -p gateway-local --lib artifacts::`, `cargo test --locked -p gateway --lib drain_tests::`, and `realtime_stt::` from `cargo test --locked -p gateway --test it --all-features` pass.
  - The ignored native suites need no rerun: neither change touches the FFI or the speech engine.
- Exit criteria:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`.
  - `cargo clippy` with `-D warnings` for the changed crates, `cargo fmt --all --check`, rustdoc with warnings denied, `cargo xtask site --books-only`, `cargo check -p gateway --no-default-features`, and `cargo test -p build-xtask`.
  - On this machine, cargo runs in WSL through `bash vibe/scratch/wsl-cargo.sh`, and clippy, fmt, and the books run with the Windows toolchain. `vibe/scratch/verify-final.sh` runs the full set except clippy for `gateway`.
  - `gateway`'s clippy runs on Windows over its headless features, because the config UI's `node_modules` here is a Linux install and the WSL toolchain has no clippy. CI's clippy on pull request #91 covers `--all-features`.

## Decision Record

- Decisions:
  - Both debts are fixed on the pull request's branch, continuing the operator's call for the whisper CUDA backend to "fix every found issue that are specific to this PR's change or regression from this PR".
  - Windows `auto` counts any non-index first entry as hiding every GPU.
    - It closes the stop crash's last `auto` path in selection, the plan's only guard, with no new probe data.
    - Its cost is the plan's existing trade: a machine `auto` cannot vouch for gets the CPU build, and `cuda` stays the override, as for Turing GPUs and old drivers.
    - It is limited to Windows, because only the Windows CUDA build crashes at a stop after a CPU fallback.
  - The retirement's warning is keyed to speech's state and to whether any time remained.
    - The retirement still runs past an exhausted deadline as a spawned task, so behavior at process exit does not change.
    - Only the diagnostic and the docs change.
- Rejected alternatives:
  - Matching identifiers against `nvidia-smi --query-gpu=uuid`: more precise, since valid UUIDs would keep the GPU, but it changes the probe's query, its parser, and the probe's answer for a low-reach path. Revisit if Windows operators with valid UUIDs need CUDA under `auto`.
  - Rebuilding the Windows CUDA archive without OpenMP: the durable cure for DB-1 and the explicit-`cuda` residual, but it needs the self-hosted runner, a new archive name, and new pins. Revisit at the next whisper.cpp tag.
  - Asking ggml for its device count after load: it needs a second library handle and new FFI symbols. Revisit if a CPU fallback must be detected at load for another reason.
  - Documenting DB-1 instead of fixing it: it leaves a process-ending path under the default setting. No revisit condition.
  - Retiring past an exhausted deadline with a small floor: it changes the documented shutdown bound. No revisit condition.
  - Skipping the retirement when nothing is published: no different for the warning, and keeping the spawn leaves the shutdown path as it is. No revisit condition.
- Assumptions, risks, and notes:
  - CUDA 13 on Windows treating an identifier that names no GPU as invalid, exposing no device, follows CUDA's documented rule and was not reproduced. The end-to-end check confirms the new selection, not CUDA's behavior.
  - A Windows operator relying on a valid UUID under `auto` moves to the CPU build. The docs state the change and the `cuda` override.
  - The DB-2 tests rely on capturing tracing output in unit tests, which the gateway crate's logging tests already do.
  - The whisper CUDA backend plan, `vibe/2026-09-28-2-whisper-cuda-backend.md`, is still the active plan until its queue drain and close.
  - Decided during the run: the Windows headless `gateway` clippy fails here with four errors from master's code. They are three `clippy::float_cmp` in `crates/gateway/app/src/tray/logic-tests.rs`, whose `#[expect]` attributes `2ea6bb6b` removed for Rust 1.99, while local clippy is 1.98. The fourth is `dead_code` for `realtime_upgrade_status` in `crates/gateway/app/src/boot-speech-tests.rs`, whose only caller is gated on `config-ui`. This plan touches neither file, so Step 2's check counts as passing when those four are the only errors. Falsifier: any error in a file this plan changes, or CI's clippy failing on pull request #91.

## Project survey

Surveyed at `a13f1ed2` (`Close plan: whisper-cuda-backend`) on `whisper-cuda-backend`, clean tree, no `vibe/ACTIVE`. Architecture anchor: `vibe/archdoc.md` (nine components, invariants A1 to A9), read whole. The previous plan's survey was the lead; each fact below was rechecked against this checkout.

- Build command:
  - `cargo build --locked -p gateway`; plain `cargo build` builds the one `default-members` crate, `crates/gateway/app`. Default features are `local`, `web-search`, `config-ui`, `stt`. `config-ui` bundles `crates/gateway/config-ui/ui` with esbuild after `npm ci --prefix crates/gateway/config-ui/ui`; that `node_modules` is present here as a Linux install (`.bin/esbuild` only, no `esbuild.cmd`), so a Windows build of `gateway-config-ui`, and so of default or all-features `gateway`, fails.
  - Headless shape: `cargo build --locked -p gateway --no-default-features`; the Windows end-to-end build is `cargo build --release --locked --offline -p gateway --no-default-features --features local,stt`. Desktop app: `cargo workshop` (alias of `run -p build-workshop --`) after `npm ci --prefix crates/workshop`, whose `node_modules` is absent here.
  - Toolchain: `rust-toolchain.toml` pins channel `stable`; edition 2024, resolver 3, no `rust-version`. Windows has cargo and rustc 1.98.0 (also 1.89 installed), WSL has 1.98.1. CI takes the current stable through `dtolnay/rust-toolchain`, and the merge `e1d58405` brought master's Rust 1.99.0 clippy fixes, so local clippy 1.98 can miss lints CI raises. Windows links with `rust-lld` and `+crt-static` (`.cargo/config.toml`).
  - Two forms on this machine:
    - WSL: `bash vibe/scratch/wsl-cargo.sh <cargo args>` runs `cargo <args>` in WSL Ubuntu-24.04 at `/mnt/d/_cppalliance/promptforge`, with `CARGO_TARGET_DIR=~/promptforge-verify-target` and `RUSTFLAGS` and `RUSTDOCFLAGS` passed through `WSLENV`. It is for `cargo test`, `check`, `doc`, and any build of the config UI. The WSL toolchain has only cargo, rustc, and rust-std: no clippy, rustfmt, nextest, or mdbook.
    - Windows (plain `cargo` from Git Bash or PowerShell): `cargo clippy`, `cargo fmt`, `cargo xtask site --books-only` (mdBook 0.4.44 on PATH at `C:\Users\Will\.cargo\bin\mdbook`, CI's pinned version, so `MDBOOK` need not be set), `cargo deny` 0.20.2, and the end-to-end scripts. No `cargo nextest` and no `cargo audit` on either side.
- Focused test command pattern:
  - CI form: `cargo nextest run --locked -p <package> --all-features [<filter>]`. Here: `bash vibe/scratch/wsl-cargo.sh test --locked -p <package> [--all-features] [--lib | --test it] [<module-path filter>]`.
  - This plan's areas:
    - `bash vibe/scratch/wsl-cargo.sh test --locked -p gateway-local --lib artifacts::` (the selection tests sit at `artifacts::assets::tests::`).
    - `bash vibe/scratch/wsl-cargo.sh test --locked -p gateway --lib drain_tests::` (the module is `runner::drain_tests`; its speech tests are `#[cfg(feature = "stt")]`, a default feature).
    - `bash vibe/scratch/wsl-cargo.sh test --locked -p gateway --test it --all-features realtime_stt::` (`mod realtime_stt` is `#[cfg(feature = "stt")]` in `tests/it/main.rs`, with twelve submodules under `tests/it/realtime_stt/`).
  - One crate's doctests: `cargo test --locked --doc -p <package>`.
  - Native whisper tests are `#[ignore]`d and run as `-- --ignored --test-threads=1` with `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO` set, as `.github/workflows/stt-miri.yml` does on the self-hosted Windows CUDA runner; this plan needs no rerun.
  - Windows end to end, from Git Bash, with the scratch scripts in `vibe/scratch/run-36900875610/scripts/`:
    - `win-env.sh` is sourced by each step: clone `vibe/scratch/pf-e2e`, target `vibe/scratch/pf-e2e-target`, `CARGO_NET_OFFLINE=true`, logs under `run-36900875610/logs/`, and a HEAD guard that still names `36409119`.
    - `win-prepare.sh` runs `git checkout -- .` in the clone, applies `STEP_PATCH` with `core.autocrlf=false`, points the Windows CPU row at `http://127.0.0.1:18741` through `patch-assets.py` (the CUDA row keeps its pin), copies both archives to `serve-windows/`, and builds the headless release gateway and the native test targets.
    - `python win_gateway.py <section>...` takes `boots`, `stops`, `soak`, `hidden`, `jit`, and `long`, and `WIN_GATEWAY_LABEL` labels its log. `hidden` sets `CUDA_VISIBLE_DEVICES` to `-1` today, and its docstring says so.
    - The clone sits at `36409119` with an earlier step's patch still in its working tree; `win-prepare.sh` discards it. The Windows machine has two RTX 3090 GPUs on driver 591.86.
- Full-suite test command:
  - CI: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; the workshop trio apart as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - Here, without nextest, as `vibe/scratch/verify-final.sh` runs it: `bash vibe/scratch/wsl-cargo.sh test --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --exclude workshop-gateway --exclude gateway-api-discovery --all-features`, then `-p workshop-gateway --all-features -- --test-threads=1` and `-p gateway-api-discovery --all-features -- --test-threads=1`.
    - Plain `cargo test` also runs the doctests, but loses `.config/nextest.toml`'s `heavy` group (8 threads, 4 per test for `gateway-stt` and `gateway-stt-backend-whisper`).
    - The two single-threaded crates avoid a pre-existing copy-then-exec ETXTBSY race under the threaded runner (https://github.com/rust-lang/rust/issues/114554).
  - Structural checks: `bash vibe/scratch/wsl-cargo.sh test --locked -p build-xtask`.
  - `bash vibe/scratch/verify-final.sh` (Git Bash) runs fmt on Windows; the tests above, `check --locked -p gateway --no-default-features`, rustdoc with `-D warnings`, and `build-xtask` in WSL; then `cargo clippy --locked -p gateway-config -p gateway-local -p gateway-stt --all-targets --all-features -- -D warnings` and `cargo xtask site --books-only` on Windows. Its header comment still describes the previous plan's Step 5.
- Linter and formatter commands:
  - CI clippy: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - Here, Windows only: `cargo clippy --locked -p gateway-local --all-targets --all-features -- -D warnings`, and for `gateway`, which cannot build the config UI on Windows, `cargo clippy --locked -p gateway --no-default-features --features local,stt,web-search --all-targets -- -D warnings`.
  - Formatter: `cargo fmt --all --check` on Windows (`rustfmt.toml`: `style_edition = "2024"`).
  - Headless gate: `bash vibe/scratch/wsl-cargo.sh check --locked -p gateway --no-default-features`.
  - Docs: `RUSTDOCFLAGS="-D warnings" bash vibe/scratch/wsl-cargo.sh doc --locked --no-deps --all-features --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api`. CI's docs job also runs `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`.
  - Books: `cargo xtask site --books-only` on Windows. The single-file guide exports come from `cargo run --locked -p build-user-guide`, which depends on no workspace crate and so runs with either toolchain.
  - CI's speech gates, with `RUSTFLAGS=-D warnings`: `cargo rustc --locked -p <crate> --lib -- -F unsafe-code` for `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-stt`, and `cargo check --locked -p gateway-whisper-ffi --lib`.
  - Supply chain: `cargo deny check` (Windows), `cargo audit` (CI only). Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (`PINNED` in `crates/build-xtask/src/api/toolchain.rs`), needed only when a facade changes; that nightly is not installed, and this plan touches no facade.
  - UI: `npm run typecheck`, `npm run build`, `npm test` in `crates/gateway/config-ui/ui`; `npm run typecheck --workspaces --if-present` and `npm test --workspaces --if-present` in `crates/workshop`.
  - `.githooks/pre-commit` (fmt) and `.githooks/pre-push` exist, but `core.hooksPath` is unset, so neither runs. Workflows have no linter in the repository.
- Test placement and naming conventions:
  - Unit tests sit with their module in one of three forms: an inline `#[cfg(test)] mod tests { ... }` at the file's end (`artifacts/assets.rs` at line 620), a kebab sibling `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;`, sometimes under a descriptive module name (`staging-tests.rs` as `tests`, `boot-speech-tests.rs` as `boot_speech_tests` under `#[cfg(all(test, feature = "stt"))]`, `main-logging-tests.rs` as `logging_tests`), or `mod tests;` in the module directory (`config/tests.rs` with `config/tests/{schema,serialize,validation}.rs`).
  - `runner.rs` holds two inline test modules: `#[cfg(test)] mod drain_tests` (line 574, with `park_a_command`, `scripted_speech`, `speech_gateway`, `both_workers_drop`, and the four `serve_*` speech tests) and `mod tests` (line 1539).
  - Log capture in unit tests: the tests of `crates/gateway/protocol/src/upstream.rs` define `LogBuffer` (a `MakeWriter` over `Arc<Mutex<Vec<u8>>>`), `capture_logs(max_level)`, and `capture_warnings()`, installed with `tracing::subscriber::set_default`. `tracing-subscriber` (with `env-filter`) is a normal dependency of `gateway`.
  - Integration tests are one binary per crate at `tests/it/main.rs` (`tests/suite/` in the `promptforge` and `harness` facades), with helpers in `tests/it/support.rs` for `gateway` and `tests/common/` for `gateway-stt`, and data in `tests/fixtures/`. A few targets stand alone, such as `crates/gateway/stt/backend-whisper/tests/native_whisper.rs` and the speech engine's `tests/engine_contract.rs`.
  - Names are snake_case sentences stating the behavior (`gpus_hidden_from_cuda_select_the_cpu_whisper_build`, `serve_retires_speech_before_it_returns`). `clippy.toml` allows `unwrap` and `expect` in tests; product code may not use them.
  - Test-only hooks sit behind `test-fixtures` (gateway family, workshop crates), `test-support` (Engine and Harness crates), or `test-helpers` (`gateway-routing`). `gateway` and `gateway-stt` dev-depend on themselves with `test-fixtures`; public `gateway-config` types carry `# Examples` doctests.
  - TypeScript and JavaScript tests are `node --test` `.mjs` files: `src/**/*.test.mjs` in the config UI, `test/*.mjs` in the Workshop `ui`, `look`, and `platform` packages, and `tools/*.test.mjs` beside their scripts.
- Directory map:
  - Languages seen: Rust, TypeScript, CSS, JavaScript (`.mjs` and `.js`), Python (`tools/cicerone/scripts/`, plus untracked scratch scripts), Lua (`crates/promptforge-internal/lua/src/__impl_*.lua` and prompt code fences), Jinja (`crates/gateway/local/src/chat_templates/assets/`), NSIS (`crates/workshop/desktop/installer.nsi`), bash (hooks, workflows), PowerShell (four workflows), TOML, YAML, JSON, HTML, Markdown.
  - `crates/` root, the public layer: `promptforge` (Engine facade), `harness` (Harness facade), `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (a TypeScript and CSS package, not a crate), `workspace-hack` (cargo-hakari), and the `build-*` tools: `build-xtask`, `build-user-guide`, `build-ui`, `build-workshop`, `build-llama-cuda`.
  - Private manifestless family containers:
    - `crates/promptforge-internal/`: `engine`, `lua`, `parser`, `model-client`, `types`, `vfs`.
    - `crates/gateway/`: `app` (package `gateway`, bin `promptforge-gateway`), `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/`: `api` (package `gateway-stt`), `engine` (`gateway-stt-engine`), `backend-whisper`, `whisper-ffi` (`gateway-whisper-ffi`).
    - `crates/workshop/`: `desktop` (package `workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, the TypeScript packages `ui`, `look`, `platform`, and `binaries/`.
    - `crates/harness-internal/`: `log`, `models`, `plugins`, `runner`, `sessions`, `web`, `webfetch`, `web-search`.
    - `Cargo.toml` lists `crates/*` and every container member explicitly, excludes the containers and `crates/shared-ui`, and sets `default-members = ["crates/gateway/app"]`.
  - `guide/`: hand-edited mdBook chapters in `guide/src/{gateway,workshop,language}/` (numbered, such as `gateway/05-speech.md`) and `guide/src/introduction.md`, plus `books/`, `chrome/`, `landing/`, `CONTRIBUTING.md`; `guide/promptforge-<set>-guide.md` are the generated single-file exports.
  - `.github/workflows/`: `ci.yml` (jobs fmt, clippy, test, docs, check-workshop, check-workshop-linux, ui, supply-chain, api-surface, ci-green), `stt-miri.yml`, `whisper-lib.yml`, `llama-cuda-blackwell.yml`, `nightly.yml`, `promptforge-gateway-v-release.yml`, `gateway-release-test.yml`, `release-workshop.yml`, `workshop-installer-smoke.yml`, `site.yml`, `dist-ci/`.
  - Tooling: `.config/` (`nextest.toml`, `hakari.toml`), `.cargo/config.toml` (aliases `xtask`, `workshop`), `.githooks/`, `.cursor/rules/`, and `tools/`: the Node sidecar staging and TTS scripts, and `tools/cicerone.md` with `tools/cicerone/` for facade pages.
  - Other trees: `prompts/` (example prompt documents), `images/`, `vibe/` (exported plans, `archdoc.md`, analyses, monthly archives `vibe/YYYY-MM/`, and the ignored `vibe/scratch/`), `target/` (ignored).
  - Root files: `gateway.local.example.toml` (the sample gateway config; `whisper_backend` at line 52), `deny.toml`, `dist-workspace.toml` (cargo-dist gateway packaging), `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.gitattributes`, `AGENTS.md`, `README.md`, `LICENSE` (BSL-1.0).
- Component boundaries (archdoc components; arrows are Cargo normal edges from `cargo metadata`, `workspace-hack` omitted):
  - Engine, Lua VM boundary, VFS layer: `promptforge` -> `promptforge-engine`, `promptforge-lua`, `promptforge-model-client`, `promptforge-parser`, `promptforge-types`, `promptforge-vfs`; `promptforge-engine` -> `promptforge-lua`, `promptforge-model-client`, `promptforge-parser`, `promptforge-types`, `promptforge-vfs`; `promptforge-parser` -> `promptforge-lua`, `promptforge-types`; `promptforge-lua` -> `promptforge-model-client`, `promptforge-types`, `promptforge-vfs`; `promptforge-model-client` -> `promptforge-types`. `promptforge-types` and `promptforge-vfs` have no dependencies. Outside crates name only `promptforge`.
  - Harness: `harness` -> `harness-log`, `harness-plugins`, `harness-runner`, `harness-sessions`, `promptforge`; `harness-sessions` -> `harness-log`, `harness-models`, `harness-plugins`, `harness-runner`, `harness-web`; `harness-web` -> `harness-plugins`, `harness-web-search`, `harness-webfetch`; `harness-runner` -> `harness-log`, `harness-plugins`. Each reaches the Engine only through `promptforge` and names no gateway, shared, or workshop crate.
  - Gateway:
    - `gateway` -> config, logging, progress, protocol, routing, `gateway-api-types`, `gateway-api-discovery`, `shared-loopback`, and the optional `gateway-local`, `gateway-stt`, `gateway-web-search`, `gateway-config-ui`.
    - Speech: `gateway-stt` -> `gateway-local`, `gateway-config`, `gateway-progress`, `gateway-stt-engine`, `gateway-stt-backend-whisper`; backend-whisper -> `gateway-whisper-ffi`, the speech engine crate, progress.
    - Below speech: `gateway-local` -> config, progress, protocol, routing, `shared-error-source`; routing -> config, protocol; web-search -> config, protocol; protocol -> config, `gateway-api-types`; progress -> `gateway-api-types`; `gateway-config` -> `gateway-api-types` only; `gateway-config-ui` -> `shared-loopback`.
    - `gateway-cloud-providers` -> `gateway-api-types`, `shared-error-source`, and no crate depends on it; it carries the `shared-cloud-providers` sheet binary.
    - Leaves: `gateway-whisper-ffi`, `gateway-stt-engine`, `gateway-logging`. No gateway crate names a promptforge, Harness, or workshop crate.
  - Workshop: `workshop` (desktop) -> `workshop-server-api`, `gateway-api-discovery`; `workshop-server-api` -> `workshop-server` -> features (`workspace`, `user-state`) -> services (`gateway`, `menu`, `status`) -> vocabulary (`protocol`, `registry`, `support`). The server also names `harness`, `promptforge`, `gateway-api-discovery`, `shared-loopback`.
  - Shared: `gateway-api-types`, `shared-error-source`, `shared-loopback` have no workspace dependencies; `gateway-api-discovery` -> `shared-error-source`. The `build-*` crates have no workspace normal dependencies; `build-ui` is a build-dependency of `workshop-server` and `gateway-config-ui`.
  - This plan's two owners: whisper selection in `gateway-local` (`crates/gateway/local/src/artifacts/assets.rs`, 1326 lines, with the probe in `artifacts.rs`, which queries `--query-gpu=compute_cap,driver_version`), and the graceful stop in `gateway`'s `Gateway::serve` (`crates/gateway/app/src/runner.rs`, 1816 lines). `serve` reaches speech through `gateway_stt::SpeechService::status()` (`crates/gateway/stt/api/src/service.rs`), whose `SpeechStatus::ready()` is in `status.rs`. Neither change crosses a crate edge.
  - Runtime: Workshop and the Harness reach the gateway over HTTP and WebSocket through the discovery file. Archdoc A1 (bind and report readiness before provisioning through the command queue) and A5 (local model set fixed for the process lifetime) govern speech provisioning; both now sit in the `## Invariants` crate doc of `crates/gateway/app/src/lib.rs`, since `vibe/archdoc.md` was retired.
  - Drift: the archdoc's CLI component has no crate here.
- Conventions summary:
  - Lints: workspace `unsafe_code = "forbid"`, clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, `missing_docs`, `unreachable_pub`, and `missing_debug_implementations` warn (fatal under `-D warnings`), and rustdoc broken and private intra-doc links deny. `gateway`, `gateway-whisper-ffi`, `gateway-api-discovery`, and the desktop app carry their own lint tables, lowering `unsafe_code` to `deny` and `pedantic` to warn. Each `unsafe` block has a `// SAFETY:` comment.
  - Layout: source directories stay flat unless they hold at least three files; one or two become kebab siblings through `#[path]`. Every crate's `build.rs` runs `build_ceiling::check()`, so a source file over 500 lines fails the build in every crate, the gateway crates included.
  - Artifacts (`gateway-local`): runtimes are digest-pinned rows (`ServerAsset`, `WhisperAsset`) in `artifacts/assets.rs`, with lowercase-hex sha256 and `<os>-<arch>[-<variant>]` platform names. Selection is a pure function over arguments (`whisper_asset_with_probe`, `auto_whisper_backend`, `cuda_visible_devices_hides_every_gpu`), so tests run on any machine. The literal `whisper-lib-b4938` appears only in `assets.rs` and `.github/workflows/stt-miri.yml` among product files.
  - Errors: `thiserror` 2 enums, `#[non_exhaustive]`, messages written for model consumption naming required versus actual. Comments state constraints only; a workaround cites its upstream issue URL.
  - Logging: `tracing` with structured fields. The gateway runs an unredacted stdout layer beside the `gateway.log` file layer (`init_logging_for_state` in `crates/gateway/app/src/main.rs`); the file layer redacts through `crates/gateway/logging/src/redact.rs`. Warnings are plain `tracing::warn!` text, such as `speech did not retire within {WORKER_JOIN_TIMEOUT:?}; abandoning it` at `runner.rs` line 537.
  - Guide: chapters are hand-edited, with no em-dash or double-dash, four-backtick fences, and one line per paragraph (`guide/CONTRIBUTING.md`); the exports come only from `cargo run -p build-user-guide`.
  - Line endings: `.gitattributes` forces LF, CRLF only for `.ps1` and `.bat`.
  - Terms: AGENTS.md's Host, Engine, Harness, and Plugin rules bind code comments, docs, and plans; the machine is "machine", the gateway's speech engine is named as such, and checks are "structural checks" or "test support". `crates/workshop/ui/test/docs-claims.mjs` enforces them in AGENTS.md files, `## Invariants` docs, and `.cursor/rules`.
  - Commits: imperative subject with no prefix, a prose paragraph on the behavior, then bullets naming the touched symbols and tests. Trailers are `Design: ...`, `Repairs: <what> @ <path>::<symbol> - <symptom>`, `Uncertain: ...`, `Deferred: ...`, and `Plan: vibe/<plan>.md`. A plan's first step commit adds the exported plan and a `vibe/ACTIVE` file naming it; a final `Close plan: <words>` commit deletes `vibe/ACTIVE`.
  - Run state: the ledger is `vibe/scratch/vibe-ledger.md` and verification logs go under `vibe/scratch/logs/`, both ignored through `vibe/.gitignore` (`/scratch`); review findings go to the repository-root `vibe-review.md`, ignored by the root `.gitignore`.
  - Remotes: `origin` is the fork `wpak-ai/promptforge`; `upstream` is `cppalliance/promptforge`, never pushed to.
  - Machine: `gh`, Python 3.12, WSL2 Ubuntu-24.04, and `nvidia-smi` with two RTX 3090 GPUs; free space is 242 GB on C:, 94 GB on D:, and 182 GB on F:.
- Rules manifest (22 tracked `AGENTS.md`, each governing its own directory's subtree):
  - `AGENTS.md`: the whole repository.
  - Root crates: `crates/gateway-api-discovery/AGENTS.md`, `crates/harness-gateway-client/AGENTS.md`, `crates/shared-loopback/AGENTS.md`, `crates/shared-ui/AGENTS.md`.
  - Gateway: `crates/gateway/{app,config,logging,progress}/AGENTS.md` and `crates/gateway/stt/{engine,whisper-ffi}/AGENTS.md`; `cloud-providers`, `config-ui`, `local`, `protocol`, `routing`, `web-search`, `stt/api`, and `stt/backend-whisper` have none. This plan's code falls under `crates/gateway/app/AGENTS.md`; `gateway-local` has none.
  - Engine: `crates/promptforge-internal/{engine,lua,types,vfs}/AGENTS.md`.
  - Harness: no harness-internal crate has an `AGENTS.md`; `harness-plugins` and `harness-runner` state their rules in their `lib.rs` `## Invariants` blocks.
  - Workshop: `crates/workshop/AGENTS.md` (the whole family and its npm workspace), `crates/workshop/{desktop,server,ui,look,platform}/AGENTS.md`, and `crates/workshop/desktop/icons/AGENTS.md`.
  - Scoped rules outside AGENTS.md: `.cursor/rules/workshop-architecture.mdc` (`crates/workshop*/**`) and `.cursor/rules/workshop-spa.mdc` (`crates/workshop/{ui,look,platform}/**`).

## Execution Instructions

- Components, in dependency order:
  1. The Windows identifier rule in whisper build selection, in `gateway-local`, with its docs, its records, and its end-to-end check, Step 1. It goes first because it removes a process-ending path under the default setting, and component 2 uses none of its code or tests.
  2. The speech retirement's warnings in `gateway`'s `Gateway::serve`, with their doc comments and records, Step 2. It goes second because it changes only a diagnostic, and as the last step it carries the exit criteria, which run once after both components land.
- Pieces:
  - The identifier rule is one piece, Step 1, built together:
    - The helper's new argument, its call in `auto_whisper_backend`, their doc comments, and the selection tests change in one commit, because the helper's new signature breaks its test until both change.
    - The docs and the whisper CUDA backend plan's identifier statements join the step, because they state its final rule and have no tests of their own.
    - The end-to-end check runs against the step's diff before its commit.
  - The retirement's warnings are one piece, Step 2, built together:
    - The status read, the deadline check, the two warnings, the doc comments, and the drain tests change in one commit, because the tests' log assertions cover exactly that branch.
    - The whisper CUDA backend plan's statements on a stop during a long load join the step, because they follow its final wording.
- Landing:
  - The work starts after the whisper CUDA backend plan, `vibe/2026-09-28-2-whisper-cuda-backend.md`, closes.
  - It lands on branch `whisper-cuda-backend`, in pull request #91, with no push to cppalliance/promptforge.
  - `/export-vibe-plan` runs before Step 1, and the plan's repository copy rides in Step 1's commit.
  - Both steps edit `vibe/2026-09-28-2-whisper-cuda-backend.md`, in disjoint passages, one after another.
  - Each step is one commit carrying its code, tests, and docs, and runs its focused tests from the Testing Plan before that commit.
  - The exit criteria run once, in Step 2, with both changes in the tree.
- Deferred and out of scope:
  - The Windows CUDA archive rebuild, and any change to `.github/workflows/whisper-lib.yml` or to a pin.
  - UUID matching, and the explicit-`cuda` residual.
  - The exposed pre-existing debt and every rejected candidate.
  - `vibe/archdoc.md` and `vibe/archdoc-next.md`.
  - The shared shutdown deadline's length or structure.

### Step 1: Keep Windows `auto` off the CUDA whisper build for unmatched device identifiers [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - `cuda_visible_devices_hides_every_gpu(value, gpu_count)` gains `identifiers_visible: bool`; when it is false, a first entry that is not a decimal device index below `gpu_count`, a `GPU-` or `MIG-` identifier included, hides every GPU.
  - Its doc says that with `identifiers_visible` true it applies CUDA's rule as today, and that false counts an identifier the probe cannot match as hiding every GPU.
  - `auto_whisper_backend` passes `os != "windows"`, with a comment that only the Windows CUDA build crashes at a graceful stop after a CPU fallback and the probe reads no UUIDs.
  - `auto_whisper_backend`'s doc adds a Windows identifier first entry to the inputs that get the CPU build.
  - Explicit `cpu` and `cuda`, `whisper_asset_with_probe`, `CudaMachine`, the probe and `machine_cuda()` in `crates/gateway/local/src/artifacts.rs`, and llama-server's selection do not change.
- Docs:
  - `guide/src/gateway/05-speech.md`, under "The runtime", says that on Windows `auto` counts a first entry that is not a device index, a `GPU-` or `MIG-` identifier included, as hiding every GPU, and that `cuda` restores GPU decoding for a valid identifier.
  - The same paragraph keeps the Linux rule, where an identifier counts as visible without being matched, and its Windows crash sentence names only an explicit `cuda`.
  - The `whisper_backend` row of `crates/gateway/app/README.md` and the commented `whisper_backend` lines of `gateway.local.example.toml` say the same.
  - `guide/promptforge-gateway-guide.md` is regenerated with `cargo run -p build-user-guide`.
  - In `vibe/2026-09-28-2-whisper-cuda-backend.md`, three statements take the Windows rule, and its completed steps stay as recorded:
    - the success criterion on `CUDA_VISIBLE_DEVICES` hiding every GPU;
    - the Functional Specification bullet on when `CUDA_VISIBLE_DEVICES` leaves a GPU visible;
    - the Decision Record entry on a `CUDA_VISIBLE_DEVICES` that hides every GPU.
- Tests, in `assets.rs`:
  - `cuda_visible_devices_hides_every_gpu_by_cudas_rule` keeps today's cases with identifiers visible.
    - With identifiers not visible and two GPUs, `GPU-8f6e2c1a-...`, `MIG-8f6e2c1a-...`, and `GPU-` hide every GPU.
    - Unset, `0`, `1`, `1,0`, and `0,-1` still leave one visible, and empty, `-1`, `2`, and `none` still hide them.
  - `gpus_hidden_from_cuda_select_the_cpu_whisper_build`: `GPU-8f6e2c1a` and `MIG-8f6e2c1a` select `windows-x86_64` on Windows and keep `linux-x86_64-cuda` on Linux.
  - `an_explicit_whisper_backend_ignores_the_probe` gains a machine whose `CUDA_VISIBLE_DEVICES` is `GPU-8f6e2c1a`, under which explicit `cuda` keeps `windows-x86_64-cuda` and explicit `cpu` keeps `windows-x86_64`.
  - Before the change, `gpus_hidden_from_cuda_select_the_cpu_whisper_build`'s Windows identifier cases fail, selecting `windows-x86_64-cuda`.
- Checks:
  - `cargo test --locked -p gateway-local --lib artifacts::` passes.
  - With the Windows toolchain, `cargo clippy --locked -p gateway-local --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`, and `cargo xtask site --books-only` pass.
  - Windows end to end, before the commit, on this Windows NVIDIA machine with the scratch scripts in `vibe/scratch/run-36900875610/scripts/`:
    - The Windows scratch clone `vibe/scratch/pf-e2e` moves to the branch head, and `win-env.sh`'s HEAD guard, now `36409119`, names that commit.
    - `win-prepare.sh`, with `STEP_PATCH` set to this step's diff, points the Windows CPU row at run 36900875610's archive and builds the headless release gateway.
    - `win_gateway.py`'s `hidden` section sets `CUDA_VISIBLE_DEVICES` to `GPU-00000000` in place of `-1`, and its docstring line says so.
    - `win_gateway.py hidden`, with `WIN_GATEWAY_LABEL=db1` so earlier logs stay, boots `b4938-windows-x86_64`, and every stop exits 0, the visible-GPU control stops included.
    - `win_gateway.py boots`, with the variable unset, still takes `b4938-windows-x86_64-cuda` on the RTX 3090s under `auto`.

### Step 2: Key the speech retirement's warnings to speech's state and the time left [completed]

- In `crates/gateway/app/src/runner.rs`, `Gateway::serve`, under `#[cfg(feature = "stt")]`:
  - Before it spawns the retirement, it reads `speech.status().ready()`, which is true only while a runtime is published with admission open.
  - It spawns `speech.shutdown()` on the blocking pool in every case, so the runtime's shutdown bound can finish it.
  - When `tokio::time::Instant::now()` has reached the shared `deadline`, it does not await the retirement, and with speech published it logs one warning, for example `speech retirement was not awaited: the command worker used the whole {WORKER_JOIN_TIMEOUT:?} deadline`.
  - With time left, it awaits the retirement until the deadline, and a published retirement still draining then logs today's `speech did not retire within {WORKER_JOIN_TIMEOUT:?}; abandoning it`.
  - With nothing published, it logs no speech warning.
- Doc comments:
  - The shutdown paragraph of `serve`'s rustdoc states the retirement's three outcomes: it finishes within the deadline, it is abandoned while still draining at the deadline, or it runs unawaited when the command worker used the whole deadline.
  - `WORKER_JOIN_TIMEOUT`'s doc says the join and the retirement share it, and that a join that spends it leaves the retirement unawaited.
  - `RUNTIME_SHUTDOWN_TIMEOUT`'s doc names the unawaited retirement beside the draining one as blocking-pool work it reaps, and both constants keep their values.
  - The comment above the retirement says the status is read first, so a stop with nothing published logs no speech warning.
- Records, in `vibe/2026-09-28-2-whisper-cuda-backend.md`:
  - The problem bullet on the stop during the 743 MB Linux CUDA download says the stop abandoned the boot command's load, and that no speech retirement was abandoned, since nothing was published.
  - The Decision Record note beginning "A stop while a CUDA speech load outlasts the shared deadline" names the abandoned load, not the retirement, as what races the CUDA runtime's teardown.
  - The Deferred residual "a stop while a speech load outlasts the deadline still abandons the retirement" says the same.
  - The guide's and the README's sentence that a stop during a load exits without retiring speech stays as written, since it names no warning.
- Tests, in `drain_tests` in `runner.rs`:
  - A test-only `capture_warnings()` installs a WARN-level `tracing_subscriber::fmt` subscriber over a shared `LogBuffer` with `tracing::subscriber::set_default`, mirroring `capture_logs` in the tests of `crates/gateway/protocol/src/upstream.rs`.
    - `#[tokio::test]`'s current-thread runtime polls the spawned `serve` on the test thread, so its warnings land in the buffer.
  - `serve_abandons_a_worker_that_ignores_cancellation_after_the_join_bound` also asserts that the speech-less gateway logs no `speech` warning, and before the change it fails on `speech did not retire`.
  - A new `serve_still_retires_idle_speech_after_a_stuck_command_spends_the_deadline`, under `#[cfg(feature = "stt")]`, parks a command with `park_a_command` on `speech_gateway` over idle `scripted_speech()`.
    - It asserts one `was not awaited` warning and no `did not retire` warning, and that `both_workers_drop` succeeds.
    - Before the change it fails on the warning.
  - `serve_abandons_a_speech_retirement_that_outlasts_the_join_bound`, whose held worker job keeps a retirement draining with time left, also asserts one `speech did not retire within` warning.
  - `serve_bounds_the_worker_join_and_the_speech_retirement_by_one_deadline` keeps its assertions; its stuck command spends the deadline, so it also asserts one `was not awaited` warning and no `did not retire` warning.
  - `serve_retires_speech_before_it_returns` also asserts that a retirement that finishes logs no speech warning.
- Checks:
  - In WSL through `bash vibe/scratch/wsl-cargo.sh`, `cargo test --locked -p gateway --lib drain_tests::` and `realtime_stt::` from `cargo test --locked -p gateway --test it --all-features` pass.
  - With the Windows toolchain, `cargo clippy --locked -p gateway --no-default-features --features local,stt,web-search --all-targets -- -D warnings` reports no error outside master's four pre-existing ones, as the Decision Record's notes state.
    - The all-features shape cannot run here, because the config UI's `node_modules` is a Linux install and the WSL toolchain has no clippy; CI's clippy job on pull request #91 covers it.
  - As the final step, it runs the exit criteria once, with both steps' changes in the tree:
    - `bash vibe/scratch/verify-final.sh` runs fmt, the workspace tests and doctests with all features, `cargo check -p gateway --no-default-features`, rustdoc with `-D warnings`, and `cargo test -p build-xtask` in WSL, then clippy for `gateway-local` and `cargo xtask site --books-only` with the Windows toolchain.
    - It uses `cargo test` in place of `cargo nextest run`, which is not installed here, as the Testing Plan's last bullet allows.
    - The `gateway` clippy above completes the changed crates' clippy.
