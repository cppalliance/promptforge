---
name: Whisper CUDA backend
overview: >-
  Give Linux x86-64 a CUDA build and Windows x86-64 a CPU build of the pinned whisper.cpp
  runtime. The existing whisper build workflow builds both and adds them to the existing
  whisper-lib-b4938 release without touching its five archives. A new [stt] whisper_backend
  setting chooses the build on those two platforms the way [local] llama_backend chooses
  llama-server: auto picks the CUDA build when nvidia-smi reports an NVIDIA GPU, and the CPU
  build otherwise. The Linux CUDA build compiles only on a release dispatch. The pull request
  merges with the two new rows fail-closed, and one commit after merge pins them.
todos:
  - id: export-vibe-plan
    content: Run /export-vibe-plan before any other step
    status: completed
isProject: false
---

# Whisper CUDA backend

## Export this plan

Before the first implementation change, run `/export-vibe-plan`. It writes this plan under each touched repository's top-level `vibe/` directory as `YYYY-MM-DD-N-words.md`.

## Product Requirements

- Problem and users:
  - The users are gateway operators on Windows x86-64 and Linux x86-64. The first is the Linux GPU host behind wg21.org's Talktron, where recognition sits on the path to a voice turn's first audio.
  - The pinned linux-x86_64 whisper.cpp runtime is the CPU build. On a 32-thread CPU, the final `small.en` pass over a 9.4 s answer takes 2.14 s, and the same sources built with CUDA take 0.13 s.
  - The only Windows x86-64 build is the CUDA build, and its CUDA backend links the NVIDIA driver library. A Windows host without an NVIDIA driver cannot load it, so it has no speech.
  - Every platform has exactly one pinned whisper build, chosen by OS and architecture alone. Nothing looks at the host's GPU.
- Goals:
  - A CUDA build for linux-x86_64 and a CPU build for windows-x86_64, from the same whisper.cpp tag, `b4938`, built and published by the existing whisper build workflow and pinned by digest like every other runtime. The pins land in one commit after this work merges.
  - One setting, `[stt] whisper_backend`, chooses the build on Windows x86-64 and Linux x86-64 the way `[local] llama_backend` chooses llama-server: `auto` detects an NVIDIA GPU, and an explicit value forces a build.
  - The C ABI, the FFI crate, the model files, and decoding stay as they are.
  - Under the default `auto`, a host the selection cannot serve gets the CPU build or a failed speech load, never an ended gateway process.
  - The native speech fixtures name their build, so no test depends on the host's GPU.
- Non-goals:
  - Replacing or rebuilding the five archives already in `whisper-lib-b4938`.
  - A CUDA build for linux-aarch64, a Vulkan whisper build, or runtime backend loading inside ggml.
  - An operator path override for the whisper library.
  - Any change to llama-server selection.
  - A config UI control for the setting.
  - Running whisper out of process.
  - Older debt this work touches but did not introduce: the Windows CUDA build's own driver floor, the MSVC runtime the Windows archives import without bundling it, `speech.gpu` reporting how a build was compiled, and the test-only eager gateway constructor.
- Success criteria:
  - The criteria that load a new build hold once the post-merge commit pins the two new rows. Until then those rows fail closed, as the Functional Specification states.
  - A Linux x86-64 host with an NVIDIA GPU and driver 570 or later, on `auto`, downloads, verifies, and loads the CUDA build: the boot log's library path names it, and `/admin/status` reports `speech.gpu` true. The final `small.en` pass takes about 0.13 s.
  - A Linux x86-64 host with an older or unreadable driver, or without an NVIDIA GPU, gets the CPU build under `auto`, as today.
  - A Windows x86-64 host with an NVIDIA GPU keeps the CUDA build. One without an NVIDIA GPU, whose CPU has the x86 baseline, loads the CPU build and transcribes.
  - A host whose CPU lacks a baseline extension fails the speech load with an error naming the required and missing extensions, and the gateway keeps serving.
  - `cpu` and `cuda` force their build on both platforms.
  - The native fixtures pass on a Linux host with an NVIDIA GPU without extra setup, and CI's native job still exercises the CUDA build.
  - A push-triggered run of the whisper build workflow does not compile the Linux CUDA build. A release dispatch compiles it and adds both new archives to `whisper-lib-b4938`, leaving every existing asset untouched.
  - Every exit gate in the Testing Plan passes.
- Constraints:
  - Every runtime download is digest-pinned, and an all-zero pin never loads.
  - Every gateway build since `whisper-lib-b4938` was published pins its five archives, and `.github/workflows/stt-miri.yml` pins the Windows CUDA one, so those archives and their `SHA256SUMS` lines never change.
  - `[stt]` is read once, at boot. It is already a restart section, so a change takes effect at the next start.
  - The GPU probe and any build download run inside speech's queued boot command, after the gateway's listener is bound and serving, as all speech provisioning does today.
  - The workspace lints hold: no `unsafe` outside the FFI crates, clippy `all` and `pedantic` at deny, and `missing_docs`, which the `-D warnings` gates make an error.
  - Gateway configuration rejects unknown fields. A gateway built before this work refuses a file that sets `whisper_backend`.
  - A release dispatch builds every row, so the self-hosted Windows CUDA runner must be online for it, as it already must be.
  - whisper runs in the gateway process, and ggml ends the process with `abort()` on a CUDA error, so `auto` must never select a CUDA build for a host whose driver cannot run it.
  - Every x86 whisper build is compiled with ggml's fixed non-native baseline, which includes AVX2. Running one on a CPU without that baseline raises an illegal-instruction fault that ends the gateway process.
- Open questions:
  - None that block. The glibc floor is 2.35, set by the hosted `ubuntu-22.04` runner, the same floor as the existing Linux archives.

## Functional Specification

- Actors and workflows:
  - A gateway operator leaves `[stt] whisper_backend` at `auto`, or sets `cpu` or `cuda`, and restarts the gateway.
  - After this work merges, anyone with write access to `cppalliance/promptforge` dispatches the whisper build workflow on `master` for the tag. One commit then pins the two new rows' digests from the release's `SHA256SUMS`.
- Inputs and outputs:
  - Input: `[stt] whisper_backend` is `auto` (the default), `cpu`, or `cuda`. Serialization omits it when it is `auto`.
  - Host inputs to selection: the NVIDIA probe's answer, which is each GPU's compute capability and the driver version, and the host CPU's instruction-set extensions.
  - Output: the whisper.cpp library speech loads, or a named selection error that fails the speech load, and one boot log line, `provisioned whisper library`, with the library's path.
    - The install directory in that path names the build, such as `b4938-linux-x86_64-cuda`. The path reaches stdout and so a service's journal, but `gateway.log` redacts local paths.
    - `GET /admin/status` reports `speech.gpu`, true when the loaded build has CUDA, whichever log is read.
  - Workflow output: `whisper-b4938-windows-x86_64.zip` and `whisper-b4938-linux-x86_64-cuda.zip` in the `whisper-lib-b4938` release, with their lines added to its `SHA256SUMS`.
- States and validation:
  - On Windows x86-64 and Linux x86-64:

    | Setting | `nvidia-smi` reports an NVIDIA GPU | No NVIDIA GPU, or the probe fails |
    | --- | --- | --- |
    | `auto` | The CUDA build. | The CPU build. |
    | `cpu` | The CPU build. | The CPU build. |
    | `cuda` | The CUDA build. | The CUDA build, which fails to load without a driver. |

  - On Linux x86-64, `auto` also requires driver 570 or later, the Linux CUDA build's floor, and an older or unreadable driver gets the CPU build. Windows x86-64 has no floor.
  - On an x86 host, every setting first requires the builds' shared CPU baseline, and a missing extension fails selection.
  - The probe runs only under `auto`, and only on those two platforms.
  - Every other platform has exactly one build, and every setting selects it. The setting is documented as consulted only on Windows x86-64 and Linux x86-64, as `[local] llama_backend` is documented as consulted only on Windows x86-64.
  - An unknown value is a configuration error that names the rejected value and the accepted ones.
  - Until the post-merge pin commit, the two new rows fail closed:
    - Under `auto`, a Linux x86-64 host with an NVIDIA GPU on driver 570 or later, and a Windows x86-64 host whose probe finds no NVIDIA GPU, select an unpinned row. Their speech load fails at the download or the digest check, and the gateway keeps serving.
    - Meanwhile `cpu` on Linux and `cuda` on Windows select today's pinned builds.
- Errors and recovery:
  - The chosen build downloads when missing and is verified against its pin. A download, verification, or load failure fails the speech load and names its stage, and the gateway never switches builds on its own.
  - Under `auto` on Linux, a driver below the floor gets the CPU build, so it never reaches the CUDA runtime.
  - An explicit `cuda` below the floor is honored, and on a GPU the build has no native code for it can end the gateway at the first transcription. The docs name the floor, with `auto` or `cpu` as the recovery.
  - A CPU missing a baseline extension fails the speech load with an error naming the required and missing extensions. Speech stays unavailable and the gateway keeps serving; no setting recovers it, because every x86 build shares the baseline.
  - As today, a failed speech load never stops the gateway and is never retried in-process. A restart is the recovery.
- Security and privacy behavior:
  - The new rows are fail-closed with all-zero digests until the post-merge commit pins them from the release's `SHA256SUMS`.
  - Publishing is add-only: an archive the release already holds is never uploaded again.
  - Publishing still happens only on a manual dispatch, and only the publish job holds `contents: write`.
- Acceptance criteria:
  - The success criteria hold, and the boot log's library path names the chosen build under each setting.

## Technical Design

- Architecture:
  - `.github/workflows/whisper-lib.yml` gains the two builds and an add-only publish. No workflow is added.
  - `gateway-config` owns the setting, and `gateway-stt`'s `prepare()` passes it through and logs the library.
  - `gateway-local` owns the GPU probe, reused from llama-server, and row selection with its host-capability checks (the driver floor and the CPU baseline), and provisioning.
  - `gateway-whisper-ffi` and `gateway-stt-backend-whisper` do not change.
  - The native speech fixtures and CI's native job name their backend explicitly.
- Modules and interfaces:
  - The Windows CPU build is a `windows-x86_64` matrix row on a hosted Windows runner, and it builds on push like the other rows.
    - It configures like the Windows CUDA row without `-DGGML_CUDA=ON`, packages like it without the CUDA runtime DLLs, and smoke-loads the same way.
    - The shared `Package Windows runtime` step fails the CUDA row when its bundle lacks a `cudart64_*.dll`, `cublas64_*.dll`, or `cublasLt64_*.dll`, the Windows counterpart of the Linux CUDA job's `ldd` check. Master's step copied them with `-ErrorAction SilentlyContinue` and never checked, so a toolkit missing one produced a bundle without it.
  - The Linux CUDA build is a job of its own in the same workflow, and it runs only on `workflow_dispatch`, so push-triggered runs skip it.
    - Runner and toolchain: hosted `ubuntu-22.04` with no GPU and no driver, and NVIDIA's CUDA 12.8 apt build components: the compiler, cudart, cuBLAS, and the driver stubs. Its timeout is sized like the hosted Blackwell CUDA build's.
    - Configure: the workflow's Linux flags plus `-DGGML_CUDA=ON`, including the `$ORIGIN` rpath. No `CMAKE_CUDA_ARCHITECTURES`, so ggml's default list applies, as it does for the Windows CUDA row.
    - Package: each ggml library once, under its soname; `libwhisper.so` under the name the gateway opens; `libcudart.so.12`, `libcublas.so.12`, and `libcublasLt.so.12` beside them, where the `$ORIGIN` rpath finds them; `whisper.h` and `LICENSE`. The zip is about 710 MB, unpacking to about 1 GB.
    - Smoke-load: `dlopen` the library and call `whisper_print_system_info()`, with the toolkit's stub `libcuda.so.1` standing in for the driver. The output must report CUDA, and `ldd` must resolve nothing outside the package except glibc's libraries, the C++ runtime, the OpenMP runtime, and the driver.
  - The publish job waits for both build jobs and becomes add-only.
    - An archive whose name the release already holds is not uploaded again, and `SHA256SUMS` gains the new archives' lines instead of being rebuilt.
    - The upload itself refuses a name the release already holds, so a filtering mistake fails the publish instead of replacing an archive.
    - A tag with no release yet is published whole, as today.
  - `WhisperBackend`, in `crates/gateway/config/src/config/stt.rs`, is `Auto` (the default), `Cpu`, or `Cuda`, shaped like `LlamaBackend`: kebab-case serde, `#[non_exhaustive]`, and `is_auto()` for `skip_serializing_if`.
    - `SttPipelineConfig` and its raw form carry `whisper_backend`, omitted from output when `auto`. The accessor is `SttPipelineConfig::whisper_backend()`.
  - `WhisperAsset.backend: Option<WhisperBackend>`, in `crates/gateway/local/src/artifacts/assets.rs`, is `Some` on the four rows of Windows x86-64 and Linux x86-64, and `None` elsewhere.
    - The new rows are `windows-x86_64` (`whisper.dll`) and `linux-x86_64-cuda` (`libwhisper.so`). Their URLs are under the existing `whisper-lib-b4938` release, and their digests are all zeros until the post-merge pin commit.
    - Until then each carries the placeholder comment that `WINDOWS_X86_64_CUDA_BLACKWELL` carried from `66f8f95f` until `56feb2dd` pinned it: the pin is filled in once the release holds the archive, and the row stays fail-closed until then.
    - `WhisperAsset` also carries an optional minimum driver version: 570 on `linux-x86_64-cuda`, the CUDA 12.8 floor, and none on `windows-x86_64-cuda`, so Windows behavior is unchanged.
  - Selection mirrors `server_asset`: `whisper_asset(os, arch, backend, gpus)` picks the row, and `whisper_asset_with_probe` runs the probe only for `auto` where both builds exist.
    - On the two platforms, `auto` becomes `Cuda` when the probe reports an NVIDIA GPU that meets the row's driver floor, and `Cpu` otherwise, including when the driver version cannot be read. An explicit value selects its own row.
    - Every other platform matches its single row.
    - Before any x86 row is returned, under every setting, the host must support every extension the pinned `GGML_NATIVE=OFF` baseline enables. The list is read from whisper.cpp `371b5a75`'s `ggml/CMakeLists.txt`, the host's side comes from `std::arch::is_x86_feature_detected!`, and other architectures skip the check.
    - A missing extension fails selection with a new `LocalError` variant naming the required and missing extensions. `LocalError` is `#[non_exhaustive]`, so the variant is additive.
    - Both checks take their inputs (the probe's answer, the host's extensions) as arguments, so tests run on any host.
  - The NVIDIA probe in `crates/gateway/local/src/artifacts.rs` reads each GPU's compute capability and the driver version in one `nvidia-smi --query-gpu=compute_cap,driver_version --format=csv,noheader` call. llama-server's selection keeps reading only the capabilities, so its behavior is unchanged.
  - `ArtifactStore::provision_whisper_library(backend, activity)`, in `crates/gateway/local/src/artifacts.rs`, still returns the library path, and goes through the existing verified install path once a row is selected.
  - `prepare()`, in `crates/gateway/stt/api/src/artifacts.rs`, reads the setting from `[stt]`, passes it to provisioning through `prepare_impl`, which takes the provisioning as an argument, and logs `provisioned whisper library` with the path, as `crates/gateway/local/src/runtime.rs` logs `provisioned llama-server`.
  - The native fixture configs in `crates/gateway/stt/api/tests/common/mod.rs`, `crates/gateway/stt/api/src/batch-native-tests.rs`, and `crates/gateway/app/tests/it/realtime_stt.rs` set `whisper_backend` from a test-only `PROMPTFORGE_WHISPER_BACKEND`, defaulting to `cpu`. It sits beside the existing `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO`, and the `native-whisper` job in `.github/workflows/stt-miri.yml` sets it to `cuda`.
- File and public API changes:
  - Changed: `.github/workflows/whisper-lib.yml`, and `.github/workflows/stt-miri.yml`'s native job with the three native fixture files.
  - New public API: `gateway_config::WhisperBackend`, `SttPipelineConfig::whisper_backend`, and the `LocalError` variant for an unsupported CPU.
  - Changed public API: `ArtifactStore::provision_whisper_library` takes the backend. Its one production caller is `prepare()`.
  - Docs:
    - `gateway.local.example.toml`: a commented `whisper_backend` line with the three values.
    - `guide/src/gateway/05-speech.md`: the two builds on Windows x86-64 and Linux x86-64, how the setting chooses between them, the Linux CUDA build's driver floor with the `auto` fallback below it, and the x86 CPU baseline. The `[stt]` row in `crates/gateway/app/README.md` and the example config state the floor and the baseline too.
    - `guide/src/gateway/04-local-models.md`: the `whisper.cpp/` cache directory, with one directory per pinned build, such as `b4938-windows-x86_64` and `b4938-linux-x86_64-cuda`.
    - `guide/promptforge-gateway-guide.md`, the assembled guide, carries the same two changes.
    - `crates/gateway/app/README.md`: the `whisper_backend` row in the `[stt]` table, matching the `[local]` table's `llama_backend` row, and the speech-runtime sentence.
  - No config UI change. The Speech card in `crates/gateway/config-ui/ui/src/pages/settings-page.ts` saves the loaded section plus its own edits, so the setting survives a save.
- Data, persistence, failure, security, and privacy constraints:
  - Cache layout: `<cache_dir>/whisper.cpp/<release>-<platform>/`, so a host's CPU and CUDA installs sit side by side.
  - The probe is one `nvidia-smi` run per speech boot, only under `auto` on the two platforms. On Windows it runs without a console window, as llama-server's probe does.
  - The setting persists only in `gateway.toml`, and it is never written when `auto`.
  - Selection is the only guard against the runtime's process-ending failures, because whisper runs in-process and ggml aborts on a CUDA error. A selection failure reaches the existing provisioning-stage speech error, which leaves speech unavailable and the gateway serving.

## Testing Plan

- Unit:
  - Config, in `crates/gateway/config/src/config/tests/`:
    - each value parses, and the canonical `[stt]` parse yields `auto`;
    - an unknown value such as `vulkan` is rejected, and the error chain names it and the accepted values;
    - the value is written only when it is not `auto`, and it serializes as its file spelling;
    - the full round-trip fixture carries `whisper_backend = "cuda"`.
  - Row selection, in `assets.rs`, mirroring the llama-server selection tests:
    - on Windows x86-64 and Linux x86-64, an NVIDIA GPU selects the CUDA build, and no GPU or a failed probe selects the CPU build;
    - `cpu` and `cuda` select their rows whatever the probe reported;
    - every other platform returns its single row under every setting;
    - the platform coverage test covers all seven rows, each a zip with a 64-hex digest under `whisper-lib-b4938`.
  - Host-capability checks, in `crates/gateway/local`, through injected inputs:
    - under `auto` on Linux x86-64, driver 569 or an unreadable version selects the CPU row, and 570 or later selects the CUDA row, while Windows x86-64 is unchanged at any driver version;
    - the probe parser reads `compute_cap, driver_version` lines, and llama-server's existing selection tests still pass;
    - a host missing any baseline extension fails selection on every x86 row under every setting, with an error naming it; a complete set selects as before, and other architectures skip the check.
- Integration and end-to-end:
  - Tests that go through provisioning, the native fixtures included, pass an explicit backend, so no test depends on the host's GPUs. The existing warm-cache reuse and no-older-ABI tests keep passing under the new signature.
  - On a Linux x86-64 host with `nvidia-smi` on PATH (WSL here), the ignored native suites pass without `PROMPTFORGE_WHISPER_BACKEND` and use the pinned CPU build: `cargo test --locked -p gateway-stt --lib -- --ignored --test-threads=1`, the same with `--test it`, and the gateway's realtime native test.
  - The Linux CUDA job's shell steps run in a scratch directory on a Linux x86-64 host before the workflow change lands, publishing nothing. WSL2 is enough.
    - Expect a successful build, a stub smoke-load that reports CUDA, and `ldd` resolving every library inside the package.
    - A container with no driver refuses the load on `libcuda.so.1`.
    - Record the host, the build time, and the architecture list ggml chose.
  - The Windows CPU row's configure, package, and smoke-load steps run on a Windows host the same way.
  - The Windows CUDA row's package step, replayed against a stub toolkit, bundles its three CUDA runtime DLLs, and with any one removed from the stub it fails naming it.
  - Optionally, a dispatch of the branch's workflow on a fork exercises both hosted jobs. The self-hosted Windows CUDA row stays queued there, so the publish never runs, and the run is cancelled. On 2026-09-30 both new builds compiled on GitHub-hosted runners this way.
  - Before landing, the add-only publish runs against a throwaway release on a fork, which needs the operator's go-ahead because it creates and deletes a public release. An archive the release already holds stays byte-identical, a new one is uploaded, `SHA256SUMS` keeps its old lines verbatim and gains the new one, and a re-run uploads nothing.
  - After merge, the release dispatch leaves the five existing archives and their lines unchanged, and the pin commit's pins equal the new lines in `SHA256SUMS`.
  - After merge, the first push run on `master` builds the Windows CPU row and skips the Linux CUDA job, and CI's `native-whisper` job passes on the CUDA build.
  - Optionally, before the pin commit merges, a gateway built from it smoke-tests each new archive on real hardware, while the selection unit tests cover `auto`'s choice between builds. A bad archive found this way is withdrawn before any merged pin names it.
    - The Linux CUDA build, on a Linux host with an NVIDIA GPU on driver 570 or later: under `auto` it loads, the log path names `b4938-linux-x86_64-cuda`, `/admin/status` reports `speech.gpu` true, and the final `small.en` pass takes about 0.13 s.
    - The Windows CPU build, on a Windows host with `whisper_backend = "cpu"`: it loads and transcribes.
- Regression, security, and performance:
  - Fail-closed needs no test of its own. No download hashes to all zeros, and the existing pin tests cover a mismatch.
  - A push-triggered run grows only by the Windows CPU row.
- Exit criteria:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`.
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`.
  - `cargo fmt --all --check`.
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, and `cargo xtask site --books-only`.
  - `cargo check -p gateway --no-default-features`.
  - `cargo test -p build-xtask`.
  - On this host, cargo runs in WSL through the Project survey's wrapper, and `workshop-gateway` runs with `--test-threads=1`, because without nextest its fixture tests race into a pre-existing ETXTBSY under a threaded runner. The WSL toolchain has no clippy, so clippy runs with the Windows toolchain for the changed crates, none of which builds the config UI.

## Decision Record

- Decisions:
  - Follow master's precedents rather than invent new policy. The user asked to "reimplement the same logic" and said "I want to follow the precedents already in master, not invent one."
  - One workflow. The whisper build workflow already builds CPU, CUDA, and Metal variants side by side into one release per tag, so the two new builds join it, and every future tag bump stays one dispatch and one release.
  - Add-only publishing is the smallest change that lets an already-published tag gain a row without replacing the archives shipped gateways pin. It also protects those pins from any accidental re-dispatch.
  - The Linux CUDA build compiles only on a release dispatch. The user asked for it ("Can we somehow make this Linux CUDA build optional on actual release only?"), and master already made its other hosted CUDA compile, the Blackwell llama-server build, manual-only.
  - A Windows CPU build in this work. The user asked for it ("Windows CPU only would be beneficial in this PR too."), and it gives a Windows host without an NVIDIA GPU working speech.
  - `auto` detects the GPU the way `[local] llama_backend = "auto"` does, reusing its `nvidia-smi` probe, and downloads the build it picks. On Linux it also requires the CUDA build's driver floor, 570, because whisper runs in-process and a CUDA error there ends the gateway, where llama-server fails as a child process.
    - Hosts with sm_86 or sm_89 GPUs on drivers 525 to 569, which could run the build, get the CPU build under `auto`, and `cuda` remains their override.
  - An explicit `cuda` is honored as given. Below the Linux floor it can end the gateway at the first transcription on a GPU without native code in the build, which the docs state.
  - Selection checks the x86 CPU baseline under every setting and fails the speech load with a named error, so a host below the baseline loses speech rather than the gateway.
    - It covers every x86 row because they share the baseline, which also closes the same exposure in the older Linux CPU and Windows CUDA rows.
  - The native fixtures read a test-only backend variable that defaults to `cpu`, and CI's native job sets `cuda`, following the existing `PROMPTFORGE_WHISPER_*` fixture variables.
  - The host-capability and fixture fixes join this plan ahead of the pins, in the user's words: "Fold these fixes into the whisper plan ahead of its checksum step." The pins land last, after merge, because they make the fixed paths reachable.
  - One pull request, #91, merges before the release, and one commit after merge pins both new rows. The user asked for it: "When the PR gets merged into upstream master after review, anyone can run the workflow, update the checksum lists in a single commit."
    - It follows master's Blackwell llama-server row. `66f8f95f` added that row with an all-zero digest and a placeholder comment, and after the first release run `56feb2dd` pinned it in one commit.
    - It replaces the dispatch from the pull request's head, which needed that head pushed to `cppalliance/promptforge` and published archives from workflow changes no reviewer had approved. That dispatch had replaced the earlier two-PR landing.
    - Consequence: until the pin commit, `master`'s `auto` sends a Linux NVIDIA host on driver 570 or later, and a Windows host whose probe finds no NVIDIA GPU, to an unpinned row whose speech load fails. `cpu` on Linux and `cuda` on Windows keep today's builds. The Blackwell row had the same window.
  - The setting lives in `[stt]`, not `[local]`. It is a speech concern read at speech boot, and `[local]` configures llama-server and the cache.
  - The CUDA runtime ships inside the Linux archive. cudart, cuBLAS, and cuBLASLt ride along, as the Windows CUDA archive's DLLs do, so a host needs a driver and no toolkit.
  - Each library is packaged once, under its soname. Copying every symlink name, as the CPU archive does, would triple the 170 MB CUDA backend.
  - ggml's default CUDA architecture list, as for the Windows CUDA row. It carries native code for sm_86, sm_89, and sm_120, and PTX for the rest.
  - CUDA 12.8 for the Linux build. It is the first toolkit with Blackwell's sm_120, and it needs driver 570 or later.
  - The provisioning log names the library path, as llama-server's does. The install directory in the path names the build, so no build-label type is needed.
  - The Windows CUDA package requires its three CUDA runtime DLLs, as the Linux CUDA job's `ldd` check requires its libraries. The user asked to include this working-tree edit "if those changes are positive and doesn't introduce any defects".
  - The upload itself enforces add-only, so the guarantee does not rest on the job's filtering alone.
    - New archives go up with `gh release upload` without `--clobber`, which refuses a name the release already holds. A filtering mistake then fails the publish instead of replacing an archive that shipped gateways and `.github/workflows/stt-miri.yml` pin.
    - `SHA256SUMS` is the one asset replaced, and its existing lines stay verbatim.
  - A published new archive is withdrawn by hand, never replaced by the workflow.
    - The new archives come from the post-merge dispatch, and one found bad before the pin commit merges follows this rule.
    - If one fails, a maintainer deletes that archive and its `SHA256SUMS` line, which is safe while no merged pin names it. The next dispatch's add-only publish uploads the fixed rebuild, whose digest the pin commit takes.
    - Once a merged pin names an archive, it never changes.
  - Real-hardware checks of the new archives leave this pull request with the pins, because the archives exist only after the post-merge dispatch. They become the optional smoke test before the pin commit merges, and the withdrawal rule, which the user confirmed with "ok to both", stays.
    - The selection unit tests cover `auto`'s choice, so forcing the Windows CPU build with `cpu` on an NVIDIA machine stands in for a Windows machine without an NVIDIA GPU.
- Rejected alternatives:
  - A separate workflow and release tag for the CUDA build: it duplicates the Linux flags and packaging and splits one tag across two releases. Revisit if a build ever needs a different whisper.cpp tag than the other rows.
  - Re-dispatching the workflow as it is: it would replace the five pinned archives with rebuilt ones whose digests differ. No revisit condition.
  - `auto` using the CUDA build only once it is installed, with a test load before committing to it: no precedent does either, and llama-server's `auto` detects and downloads. Revisit if GPU hosts commonly want the CPU build, which `cpu` already gives them.
  - Runtime backend loading inside ggml, with one archive per platform: PromptForge's own builds do not use it, and whisper.cpp at `b4938` does not load backends itself, so it would need new FFI surface. Revisit if whisper.cpp starts loading its backends itself.
  - Compiling the Linux CUDA build on every push: the hosted compile is slow, and master already keeps its hosted Blackwell CUDA compile manual. Revisit if a self-hosted Linux runner becomes available.
  - A pull-request trigger for the workflow: it has none, and changes are checked by local dry runs before landing and by the push run after. No revisit condition.
  - An operator path override for the whisper library: llama-server has one (`llama_server_path` and `PROMPTFORGE_LLAMA_SERVER`), but it is a separate capability this work does not need, since both new builds are published. Revisit if an operator needs a build this repository does not publish.
  - A warning when the setting is set on a platform with one build: `llama_backend` has none, and the docs say where the setting is consulted. No revisit condition.
  - A config UI control: `llama_backend` has none either, and the Speech card already preserves the field. Revisit if the UI gains controls for build selection generally.
  - Matching the GPU's compute capability against the archive's native-code list: it couples selection to device code frozen at dispatch. Revisit if a second CUDA build with a different list is added.
  - Adding more native CUDA architectures: it departs from the Windows CUDA row, grows the archive, and cannot change after the dispatch. Revisit at the next whisper.cpp tag.
  - Refusing an explicit `cuda` below the floor: it blocks native-code GPUs that run on drivers 525 to 569. Revisit if crashes under an explicit `cuda` are reported.
  - A lower Windows CPU baseline: slower on every host, different from the other x86 rows, and frozen at dispatch. No revisit condition.
  - Documentation alone for either crash: it leaves the process-ending path in place. No revisit condition.
  - A blanket `cpu` in the fixtures: CI's native job would fail until the Windows CPU row is pinned, and that runner's coverage would move to the CPU build. No revisit condition.
  - Running whisper out of process: a large redesign for two crash paths that selection can avoid. Revisit if another in-process abort path appears.
  - A separate cleanup plan run after this one closes: the user chose to fold the fixes in. No revisit condition.
  - Dispatching from the pull request's head before merge and pinning inside the pull request: replaced at the user's request. No revisit condition.
  - `auto` skipping an unpinned row, to close the window before the pin commit: master has no such rule, and one commit closes the window. Revisit if the window outlasts the first release dispatch after merge.
- Assumptions, risks, and notes:
  - The hosted Linux CUDA compile is slow. The hosted Windows CUDA compile took 95 minutes before that row moved to the self-hosted runner.
  - At the post-merge release dispatch, a failed build publishes nothing, because publishing waits for every build, and the dispatch is re-run after a fix.
  - A release dispatch rebuilds the five existing rows too, and the add-only publish discards them.
  - The post-merge dispatch rebuilds the Windows CUDA row under Step 9's check. The published Windows CUDA archive is 523 MB, which only a bundled cuBLAS explains, so the self-hosted runner's toolkit likely holds all three DLLs. If it lacks one, the dispatch fails at that check and publishes nothing.
  - `auto` depends on `nvidia-smi` being on the gateway's PATH. WSL2 keeps it in `/usr/lib/wsl/lib`, which systemd's default service PATH lacks, so a WSL2 service needs `cuda` set, or that directory on its PATH.
  - A card without native code compiles the PTX once, at first load. That start is slow, and it was not measured.
  - The CUDA 12.8 floor is driver 570, and CUDA's minor-version compatibility from driver 525 cannot compile newer PTX, per NVIDIA's compatibility documentation as read on 2026-09-29. Neither crash was reproduced on a host.
  - The implementation reads the baseline extensions from the pinned ggml CMake rather than assuming a list.
  - Older debt stays recorded and unfixed:
    - the Windows CUDA build's own driver floor, since that archive's toolkit, possibly CUDA 13 with floor 580, is unverified; the per-row floor then makes it a one-value change;
    - the MSVC runtime that the Windows archives import without bundling it;
    - `speech.gpu` reporting how a build was compiled even when ggml finds no device;
    - the test-only eager gateway constructor that provisions speech before binding.
  - The `workshop-gateway` fixture race is pre-existing and tracked apart from this plan.
  - The post-merge release needs someone with write access to `cppalliance/promptforge` to dispatch it, and the self-hosted Windows CUDA runner online.
  - wg21-website's Talktron gateway-host guide, `docs/talktron-gateway.md`, depends on this work.
    - Today it installs the CUDA build by setting `cuda` and returning to `auto`. With detection, a native Linux host with driver 570 or later leaves `auto`, and the WSL2 reference host sets `cuda` or extends the unit's PATH.
    - Its checks read the `provisioned whisper library` line for the build. The path names it on stdout and in the journal but is redacted in `gateway.log`, where `/admin/status`'s `speech.gpu` is the check that holds.
    - Its revision table must name the post-merge pin commit, so the website PR that carries it lands after that commit.
  - Local branches `whisper-lib-linux-cuda` and `whisper-cuda-variant` already exist and are not part of this work. New branches use other names.

## Project survey

Surveyed at `a7e50ec5` on `whisper-cuda-backend` (clean tree). Architecture anchor: `vibe/archdoc.md` (components, invariants A1 to A9), read whole.

- Build command:
  - `cargo build --locked -p gateway`; plain `cargo build` builds the same `default-members` crate, `crates/gateway/app`. Default features are `local`, `web-search`, `config-ui`, `stt`; `config-ui` bundles `crates/gateway/config-ui/ui` with esbuild, so `npm ci --prefix crates/gateway/config-ui/ui` must have run (its `node_modules` is present on this host).
  - Headless shape: `cargo build --locked -p gateway --no-default-features`. Desktop app: `cargo workshop` (alias of `cargo run -p build-workshop --`) after `npm ci --prefix crates/workshop` (its `node_modules` is absent on this host).
  - Toolchain: stable per `rust-toolchain.toml` (cargo and rustc 1.98.0 here), edition 2024, resolver 3, no `rust-version` declared. Windows links with `rust-lld` and `+crt-static` (`.cargo/config.toml`).
  - On this host every cargo command runs in WSL2 through `bash vibe/scratch/wsl-cargo.sh <cargo args>` (Ubuntu-24.04, cargo 1.98.1, target directory `~/promptforge-verify-target`, with `RUSTFLAGS` and `RUSTDOCFLAGS` passed through). `crates/gateway/config-ui/ui/node_modules` is a Linux install that the operator's WSL Talktron gateway build uses, so a Windows build of `gateway-config-ui` finds no `esbuild.cmd` and fails; the operator chose to keep it. `cargo xtask site --books-only` still runs on Windows, where mdBook 0.4.44 is installed.
- Focused test command pattern:
  - CI form: `cargo nextest run --locked -p <package> --all-features [<filter>]`. cargo-nextest is not installed on this host (`cargo nextest` is "no such command"), so run `cargo test --locked -p <package> --all-features [--lib | --test it] [<module-path filter>]`.
  - This plan's areas: `cargo test --locked -p gateway-config --lib -- config::tests:: config::stt::tests::` (submodules `validation::`, `schema::`, `serialize::`, plus the inline tests in `config/stt.rs`, which `config::tests::` alone does not match), `cargo test --locked -p gateway-local --lib artifacts::assets::tests::`, `cargo test --locked -p gateway-local --lib artifacts::tests::`, `cargo test --locked -p gateway-stt --all-features --lib artifacts::tests::`.
  - One crate's doctests: `cargo test --locked --doc -p <package>`.
  - Native whisper tests are `#[ignore]`d; they run as `-- --ignored --test-threads=1` with `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO` set, as `.github/workflows/stt-miri.yml` does on the self-hosted Windows CUDA runner.
- Full-suite test command:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; the workshop trio runs apart as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - Without nextest: `cargo test --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` also runs the doctests, but loses `.config/nextest.toml`'s `heavy` group that throttles `gateway-stt` and `gateway-stt-backend-whisper`.
  - Without nextest, `workshop-gateway`'s fixture tests also race into ETXTBSY (`Text file busy`) under the threaded runner, a pre-existing copy-then-exec race (https://github.com/rust-lang/rust/issues/114554). So that crate runs apart with `--test-threads=1`.
  - Structural harness: `cargo test -p build-xtask`.
- Linter and formatter commands:
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - `cargo fmt --all --check` (`rustfmt.toml`: `style_edition = "2024"`). Headless gate: `cargo check -p gateway --no-default-features`.
  - On this host the WSL toolchain has no clippy or rustfmt component. So `cargo fmt --all --check`, and clippy for crates that do not build the config UI, such as `gateway-config`, `gateway-local`, and `gateway-stt`, run with the Windows toolchain.
  - Docs: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; CI also runs `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`. Books: `cargo xtask site --books-only`, which runs `$MDBOOK` (default `mdbook`, CI pins mdBook 0.4.44); mdbook is not installed here, so point `MDBOOK` at a binary.
  - CI's STT gates: `RUSTFLAGS="-D warnings" cargo rustc --locked -p <crate> --lib -- -F unsafe-code` for `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-stt`; `RUSTFLAGS="-D warnings" cargo check --locked -p gateway-whisper-ffi --lib`.
  - Supply chain: `cargo deny check` (installed here), `cargo audit` (CI). Facade surface: `cargo +nightly-2026-09-05 xtask api --check`, needed only when the `promptforge` facade changes; that nightly is not installed here.
  - UI: `npm run typecheck`, `npm run build`, `npm test` in `crates/gateway/config-ui/ui`; `npm run typecheck --workspaces --if-present` and `npm test --workspaces --if-present` in `crates/workshop`.
  - Workflows have no linter (no actionlint or shellcheck in the repository); dry runs and push runs check them.
  - `.githooks/pre-commit` (fmt) and `.githooks/pre-push` (headless check, clippy, cargo deny) exist, but `core.hooksPath` is unset here, so neither runs.
- Test placement and naming conventions:
  - Unit tests live with their module in one of three forms: an inline `#[cfg(test)] mod tests { ... }` at the end of the file (`artifacts/assets.rs`, `config/stt.rs`); a kebab sibling, `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;` (`staging-tests.rs`, `gguf-tests.rs`); or `mod tests;` in the module directory (`artifacts/tests.rs`; `config/tests.rs` with `config/tests/{schema,serialize,validation}.rs`).
  - Integration tests are one binary per crate at `tests/it/main.rs` (`tests/suite/main.rs` in the `promptforge` and `harness` facades), with `tests/common/` helpers and `tests/fixtures/` data; the gateway's is `cargo test -p gateway --test it`. A few targets stand alone, such as `crates/gateway/stt/backend-whisper/tests/native_whisper.rs`.
  - Names are snake_case sentences stating the behavior (`provision_whisper_library_reuses_a_verified_install`, `no_nvidia_gpu_selects_vulkan`). `clippy.toml` lets tests `unwrap` and `expect`; product code may not.
  - Test-only hooks sit behind `test-fixtures`, `test-support`, or `test-helpers` features; `gateway` and `gateway-stt` dev-depend on themselves with `test-fixtures`. Public `gateway-config` types carry `# Examples` doctests.
  - TypeScript and JavaScript tests are `node --test` `.mjs` files: `src/**/*.test.mjs` in the config UI, `test/**/*.mjs` plus `src/**/*.test.mjs` in the Workshop UI, and `tools/*.test.mjs` beside their scripts.
- Directory map:
  - Languages seen: Rust, TypeScript, CSS, JavaScript (`.mjs`), Python (`tools/scripts/`, workflow smoke-loads), PowerShell and bash (workflows, hooks), Lua (prompt code fences, vendored `mlua`), Jinja (`crates/gateway/local/src/chat_templates/`), TOML, YAML, Markdown.
  - `crates/` root, the public layer: `promptforge` (engine facade), `harness` (harness facade), `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS package, not a crate), `workspace-hack` (cargo-hakari), and the `build-*` tools (`build-xtask`, `build-user-guide`, `build-ui`, `build-workshop`, `build-llama-cuda`).
  - Private manifestless family containers:
    - `crates/promptforge-internal/`: `engine`, `lua`, `parser`, `model-client`, `types`, `vfs`.
    - `crates/gateway/`: `app`, `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` (`api`, `engine`, `backend-whisper`, `whisper-ffi`).
    - `crates/workshop/`: `desktop`, `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, and the TypeScript packages `ui`, `look`, `platform`.
    - `crates/harness-internal/`: `capabilities`, `log`, `models`, `runner`, `sessions`, `web`, `webfetch`, `web-search`.
    - `Cargo.toml` enumerates every member explicitly and sets `default-members = ["crates/gateway/app"]`.
  - `guide/`: mdBook chapters in `guide/src/{gateway,workshop,language}/` (numbered) and `guide/src/introduction.md`, plus `books/`, `chrome/`, `landing/`; `guide/promptforge-<set>-guide.md` are generated single-file exports.
  - `.github/workflows/`: `ci.yml` (fmt, clippy, test, docs, check-workshop, check-workshop-linux, ui, supply-chain, api-surface, ci-green), `stt-miri.yml`, `whisper-lib.yml`, `llama-cuda-blackwell.yml`, `nightly.yml`, `promptforge-gateway-v-release.yml`, `gateway-release-test.yml`, `release-workshop.yml`, `workshop-installer-smoke.yml`, `site.yml`, `dist-ci/`; `.github/fixtures/` holds the workshop package smoke configs.
  - Tooling: `.config/` (nextest, hakari), `.cargo/config.toml` (aliases `cargo xtask`, `cargo workshop`), `.githooks/`, `tools/` (Node sidecar staging and TTS scripts; Python in `tools/scripts/`).
  - Other trees: `prompts/` (example prompt documents), `images/`, `vibe/` (exported plans, `archdoc.md`, analyses, monthly archives `vibe/YYYY-MM/`), `target/` (ignored).
  - Root files: `gateway.local.example.toml` (sample gateway config), `deny.toml`, `dist-workspace.toml` (cargo-dist gateway packaging), `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `AGENTS.md`, `README.md`, `LICENSE` (BSL-1.0).
- Component boundaries (archdoc components; arrows are Cargo normal edges read from `cargo metadata`):
  - Executor, Lua VM boundary, VFS layer: `promptforge` -> `promptforge-engine` -> parser, lua, model-client, vfs, types; parser -> lua; lua -> model-client, vfs. `promptforge-types` has no dependencies and `promptforge-vfs` is std-only. Outside crates name only `promptforge`.
  - Harness: `harness` -> `harness-sessions`, `harness-runner`, `harness-log`; sessions -> capabilities, models, runner, web; web -> webfetch, web-search. Every harness crate reaches the engine only through `promptforge` and names no gateway or workshop crate.
  - Gateway:
    - `gateway` (bin `promptforge-gateway`) -> config, logging, progress, protocol, routing, `gateway-api-types`, `gateway-api-discovery`, `shared-loopback`, and the optional `gateway-local`, `gateway-stt`, `gateway-web-search`, `gateway-config-ui`.
    - Speech: `gateway-stt` -> `gateway-local`, `gateway-config`, `gateway-progress`, `gateway-stt-engine`, `gateway-stt-backend-whisper`; backend-whisper -> `gateway-whisper-ffi`, engine, progress.
    - Below speech: `gateway-local` -> config, progress, protocol, routing, `shared-error-source`; routing -> config, protocol; protocol -> config, `gateway-api-types`; `gateway-config` -> `gateway-api-types` only.
    - Leaves: `gateway-whisper-ffi`, `gateway-stt-engine`, and `gateway-logging` have no workspace dependencies. No gateway crate names a promptforge, harness, or workshop crate.
  - Workshop: `workshop` (desktop) -> `workshop-server-api` -> `workshop-server` -> features (`workspace`, `user-state`) -> services (`gateway`, `menu`, `status`) -> vocabulary (`protocol`, `registry`, `support`). The server also names `harness`, `promptforge`, `gateway-api-discovery`, `shared-loopback`.
  - Shared: `gateway-api-types`, `shared-error-source`, and `shared-loopback` have no workspace dependencies; `gateway-api-discovery` depends only on `shared-error-source`. The `build-*` crates depend on no workspace crate; `build-ui` is a build-dependency of `workshop-server` and `gateway-config-ui`.
  - Runtime: Workshop and the harness reach the gateway over HTTP and WebSocket through the discovery file. Archdoc A1 (bind and report readiness before provisioning, which runs through the command queue) and A5 (local model set fixed for the process lifetime) govern speech provisioning.
  - Drift: the archdoc's CLI component has no crate here.
- Conventions summary:
  - Lints: `unsafe_code = "forbid"` workspace-wide; only `gateway`, `gateway-whisper-ffi`, `gateway-api-discovery`, and the desktop app carry their own lint tables, lowering it to `deny` and `pedantic` to warn (fatal under `-D warnings`). Each `unsafe` block has a `// SAFETY:` line. Clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, `missing_docs` warns. Members inherit `[lints] workspace = true` and `workspace-hack.workspace = true`.
  - Layout: source directories stay flat unless they hold at least three files; one or two become kebab siblings through `#[path]` (`server-support.rs`, `profile-name.rs`). The 500-line ceiling binds the 18 crates whose `lib.rs` carries `## Invariants` (every workshop-* and harness-internal crate) and `build-xtask` by its own invariant. Gateway crates carry no marker, so `artifacts.rs`, `runtime.rs`, and the test files legitimately run past 500 lines.
  - Config (`gateway-config`): public types are `#[non_exhaustive]` with private fields and `#[must_use]` accessors. Each section parses through a `pub(crate)` `RawX` with `#[serde(default, deny_unknown_fields)]`, validates in `TryFrom<RawX>`, serializes back through `From<&X> for RawX`, and routes `Deserialize` through the raw form. Enums take kebab or lowercase spellings, and a default value skips serialization through a predicate such as `LlamaBackend::is_auto`.
  - Artifacts (`gateway-local`): runtimes are digest-pinned rows (`ServerAsset`, `WhisperAsset`) in `crates/gateway/local/src/artifacts/assets.rs`, with lowercase-hex sha256 and `<os>-<arch>[-<variant>]` platform names. The literal `whisper-lib-b4938` appears only there and in `.github/workflows/stt-miri.yml`, which pins the Windows CUDA archive's hash. Windows child processes use `CREATE_NO_WINDOW`.
  - Errors: `thiserror` enums, `#[non_exhaustive]`, messages written for model consumption that name required versus actual. Comments state constraints only; a workaround cites its upstream issue URL.
  - Logging: `tracing` with structured fields. The gateway runs an unredacted stdout layer beside the `gateway.log` file layer (`init_logging_for_state` in `crates/gateway/app/src/main.rs`); the file layer never formats a classified field, `path` included, and replaces unlabeled local paths (`crates/gateway/logging/src/redact.rs`), so `provisioned llama-server`'s `path` reaches stdout but not `gateway.log`.
  - Workflows: top-level `permissions: contents: read`, with `contents: write` only on a publish job gated to `workflow_dispatch`. Third-party actions are pinned by commit SHA with a version comment, `actions/*` by major tag. Self-hosted jobs never run fork pull-request code. The hosted CUDA compile is dispatch-only (`llama-cuda-blackwell.yml`: `windows-2022`, `timeout-minutes: 240`).
  - Guide: chapters are hand-edited, with no em-dash or double-dash, four-backtick fences, and one line per paragraph (`guide/CONTRIBUTING.md`); the `guide/promptforge-<set>-guide.md` exports come only from `cargo run -p build-user-guide`.
  - Line endings: `.gitattributes` forces LF, CRLF for `.ps1` and `.bat`.
  - Commits: imperative subject with no prefix, a prose paragraph on the behavior, then bullets naming the touched symbols and tests. Trailers are `Design: ...`, `Repairs: <what> @ <path>::<symbol> - <symptom>`, and `Plan: vibe/<plan>.md`. The first step's commit adds the exported plan and a `vibe/ACTIVE` file naming it; a final `Close plan: <words>` commit deletes `vibe/ACTIVE`.
  - Run state: the ledger is `vibe/scratch/vibe-ledger.md` and verification logs go under `vibe/scratch/logs/`, both ignored through `vibe/.gitignore` (`/scratch`); review findings go to the repository-root `vibe-review.md`, which the root `.gitignore` ignores. None of them is tracked, because the repository no longer keeps run ledgers in version control.
  - Remotes: `origin` is the fork `wpak-ai/promptforge`; `upstream` is `cppalliance/promptforge`.
  - Host tooling: `gh`, `python`, WSL2 Ubuntu-24.04, `nvidia-smi` with two RTX 3090 GPUs, and CMake only inside Visual Studio 18 (`C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe`, not on PATH).
- Rules manifest (32 tracked `AGENTS.md`, each governing its own directory's subtree):
  - `AGENTS.md`: the whole repository.
  - Root crates: `crates/gateway-api-discovery/AGENTS.md`, `crates/shared-loopback/AGENTS.md`, `crates/shared-ui/AGENTS.md`.
  - Gateway: `crates/gateway/{app,config,local,logging,progress,protocol,routing,web-search}/AGENTS.md` and `crates/gateway/stt/{api,engine,whisper-ffi}/AGENTS.md`; `cloud-providers`, `config-ui`, and `stt/backend-whisper` have none.
  - PromptForge: `crates/promptforge-internal/{engine,lua,model-client,parser,types,vfs}/AGENTS.md`.
  - Harness: `crates/harness-internal/{models,sessions,web-search,webfetch}/AGENTS.md`; every harness-internal crate also states its rules in its `lib.rs` `## Invariants` block.
  - Workshop: `crates/workshop/AGENTS.md` (the whole family and its npm workspace), `crates/workshop/{desktop,server,ui,look,platform}/AGENTS.md`, and `crates/workshop/desktop/icons/AGENTS.md`.
  - Scoped rules outside AGENTS.md: `.cursor/rules/workshop-architecture.mdc` (`crates/workshop*/**`) and `.cursor/rules/workshop-spa.mdc` (the ui, look, and platform packages).

## Execution Instructions

- Components, in dependency order:
  1. The whisper release pipeline in `.github/workflows/whisper-lib.yml`, Steps 1 to 3. It goes first because it has the longest lead and needs no code: the release dispatch after merge needs it on `master`, someone with write access, and the self-hosted Windows CUDA runner online, and the pins wait on that dispatch's `SHA256SUMS`.
  2. The `[stt] whisper_backend` setting in `gateway-config`, Step 4. It comes before selection because the whisper rows and `provision_whisper_library` take its `WhisperBackend` type.
  3. Backend-aware whisper provisioning in `gateway-local` and `gateway-stt`, with its docs, the explicit fixture backend, the host-capability gate, and the markers on the two unpinned rows, Steps 5 to 8. It comes after the setting because it uses the setting's type, and its new rows install only once the post-merge commit pins their archives.
  4. The Windows CUDA package's runtime check in `.github/workflows/whisper-lib.yml`, Step 9. It returns to the release pipeline after the other components, because the user added it once Steps 1 to 8 were planned, and nothing depends on it.
- Pieces:
  - The pipeline's three pieces, Steps 1 to 3, are built one after another, add-only publish first. Each has its own check, and with the publish first no commit pairs the new builds with a publish that replaces archives.
  - The setting, Step 4, is one piece, covered by the config tests.
  - Selection, provisioning, and `prepare()` are built together as Step 5, because the new `whisper_asset` and `provision_whisper_library` signatures break the provisioning tests and `prepare()` until all of them change. The docs join that step because they describe its behavior.
  - The fixes after Step 5 are two pieces, built one after another because neither uses the other's code:
    - The explicit fixture backend, Step 6, goes first. It takes the native suites off the host's GPUs, so Step 7 can run them on this Linux GPU host as the real-host check of its CPU detection.
    - The host-capability gate with its docs, Step 7, is one piece. The driver floor and the baseline check change the same `whisper_asset` signature and the same selection tests, which building them apart would rewrite twice.
  - The markers, Step 8, follow alone. They leave the two rows for the post-merge pin commit, which makes the paths Steps 6 and 7 fix reachable.
  - The runtime check, Step 9, is one piece. Its edit already sits uncommitted in the working tree, so the step verifies and commits it.
- Landing:
  - The work happens on branch `whisper-cuda-backend`, fast-forwarded to `master` at `a7e50ec5` before Step 1. `/export-vibe-plan` runs before Step 1.
  - `/export-vibe-plan` runs again before Step 6, and again before Step 8. Each time it rewrites the plan's repository copy at its existing path, `vibe/2026-09-28-2-whisper-cuda-backend.md`.
  - Every step reaches `master` in one pull request, #91 on `cppalliance/promptforge`, which merges after review with the two new rows fail-closed.
  - No step depends on the release, because the new rows' zero digests keep them fail-closed.
  - After merge, anyone with write access runs the release and pins both rows in one commit, as the Deferred list sets out. Until then `master` selects an unpinned row where the Functional Specification says, and never a build that can end the gateway.
  - Every step passes the Testing Plan's exit gates, and its commit carries its tests. A workflow step's dry-run results are recorded under that step in the plan's repository copy.
- Deferred and out of scope:
  - The post-merge release and pin commit, for anyone with write access to `cppalliance/promptforge`:
    - Dispatch `whisper-lib.yml` on `master` with `whisper_tag` `b4938`, with the self-hosted Windows CUDA runner online. The add-only publish adds the two new archives and their `SHA256SUMS` lines.
    - Confirm that the five existing archives and their `SHA256SUMS` lines are unchanged.
    - In one commit, replace the two all-zero `sha256` values in `WHISPER_ASSETS` with the new `SHA256SUMS` lines and drop the two placeholder comments, as `56feb2dd` did for the Blackwell row. The seven-row coverage test still passes.
    - A new archive found bad before that commit follows the Decision Record's withdrawal rule.
  - Updating the wg21-website guide. That belongs to the website work.
  - A linux-aarch64 CUDA build, a Vulkan whisper build, runtime backend loading, llama-server selection, an operator library override, and a config UI control.
  - A future whisper.cpp tag bump. It publishes a new release whole, and the add-only publish does not block it. It also re-reads Step 7's x86 baseline list from the new tag's ggml.
  - The older debt listed under the Decision Record's notes, running whisper out of process, and further changes to `whisper-lib.yml`'s build steps or its CUDA architecture list beyond Steps 2, 3, and 9.

### Step 1: Make the whisper release publish add-only [completed]

- In `.github/workflows/whisper-lib.yml`, the `publish` job keeps its `workflow_dispatch` gate, its `contents: write`, which no other job holds, and its `whisper-*-${{ env.WHISPER_TAG }}` artifact download.
- Its `Assemble SHA256SUMS` and `Publish the release` steps become one add-only publish, as the Decision Record sets out:
  - With no `whisper-lib-${WHISPER_TAG}` release yet, it publishes every archive and a fresh `SHA256SUMS`, not marked latest, as today.
  - Otherwise it lists the release's assets and keeps only the archives whose names the release lacks. It appends their `sha256sum` lines to the release's `SHA256SUMS`, uploads them without `--clobber`, and then replaces `SHA256SUMS`. With nothing new, it uploads nothing.
  - The logic is one shell script that reads only `GH_TOKEN`, `GITHUB_REPOSITORY`, `WHISPER_TAG`, and `dist/`, so it runs unchanged outside Actions.
- Checks:
  - Before the commit, the publish logic runs against a throwaway release on a fork. The run asks the operator first, since it creates and deletes a public release. An archive the release already holds stays byte-identical, a new one is uploaded, `SHA256SUMS` keeps its old lines verbatim and gains the new one, and a re-run uploads nothing.
  - After the post-merge release dispatch and before the pin commit, the five existing archives and their `SHA256SUMS` lines are unchanged.
- Dry run, 2026-09-28:
  - An offline stub suite over the extracted publish script passes in Git Bash and in WSL, and three deliberately broken copies of the script fail it.
  - Against a throwaway release on the fork, the first run created the release, and the second left the held archives byte-identical, uploaded the new archive, and kept `SHA256SUMS`'s old lines verbatim with the new line appended. A re-run uploaded nothing, and the throwaway release and its tag were deleted.
  - The script also refuses a release whose archives and `SHA256SUMS` lines disagree. `whisper-lib-b4938`'s five archives and its five `SHA256SUMS` lines agree.

### Step 2: Build a Windows x86-64 CPU runtime [completed]

- The `build` job's matrix gains `platform: windows-x86_64` on `windows-2022`, the hosted image `.github/workflows/llama-cuda-blackwell.yml` builds on. It builds on push like the other rows.
- A new `Configure Windows CPU` step uses the `Configure Windows CUDA` flags without `-DGGML_CUDA=ON`.
- `Package Windows runtime` and `Smoke-load Windows runtime` run on both Windows rows, and only the CUDA row copies the CUDA runtime DLLs. `Package Unix runtime` and `Smoke-load Unix runtime` skip both Windows rows.
- Checks:
  - Before the commit, the row's configure, package, and smoke-load steps run under the ignored `vibe/scratch/` on a Windows host. The zip holds `whisper.dll`, the `ggml*.dll` libraries, `whisper.h`, and `LICENSE`, and no CUDA DLL, and the smoke-load succeeds.
  - The first push run on `master` after Steps 1 to 3 land builds the row, and no other row changes.
- Dry run, 2026-09-28, on the operator's Windows host with Visual Studio 18's CMake and MSVC 19.51 (the hosted runner has Visual Studio 2022):
  - The row's configure, build, package, and smoke-load steps all ran, and the smoke-load printed CPU-only system information (AVX2, OpenMP).
  - The zip holds `whisper.dll`, `ggml.dll`, `ggml-base.dll`, `ggml-cpu.dll`, `parakeet.dll`, `whisper.h`, and `LICENSE`, with no CUDA DLL although the host has `CUDA_PATH` set, and its `.sha256` line matches.
  - A replay of the CUDA row's package step against a stub toolkit bundles exactly what it did before the change.
  - Tag `b4938` (the same commit as `v1.9.3`) also builds a `parakeet.dll` that `whisper.dll` does not import. The shared Windows `*.dll` packaging bundles it into both Windows archives, as the published CUDA archive already holds it, and the Unix filter leaves it out.

### Step 3: Build a Linux x86-64 CUDA runtime on release dispatch only [completed]

- A new `build-linux-cuda` job in `.github/workflows/whisper-lib.yml` runs only when `github.event_name == 'workflow_dispatch'`, on `ubuntu-22.04`, with `timeout-minutes: 240`, as the hosted Blackwell build has.
- Its steps:
  - Check out whisper.cpp and clean staging as the `build` job does.
  - Install NVIDIA's CUDA 12.8 apt components: the compiler, cudart, cuBLAS, and the driver stubs.
  - Configure with the `Configure Linux` flags plus `-DGGML_CUDA=ON` and no `CMAKE_CUDA_ARCHITECTURES`, then build and install to `stage`.
  - Package `whisper-${WHISPER_TAG}-linux-x86_64-cuda.zip` and its `.sha256`: each ggml library once under its soname, `libwhisper.so` under the name the gateway opens, `libcudart.so.12`, `libcublas.so.12`, `libcublasLt.so.12`, `whisper.h`, and `LICENSE`.
  - Smoke-load `libwhisper.so` with the toolkit's stub `libcuda.so.1`. `whisper_print_system_info()` must report CUDA, and `ldd` must resolve nothing outside the package except glibc's libraries, the C++ runtime (`libstdc++` and `libgcc_s`, which the CPU archive already needs), the OpenMP runtime, and the driver.
  - Upload the archive as `whisper-linux-x86_64-cuda-${WHISPER_TAG}`, which the publish job's download pattern matches.
- The `publish` job's `needs` gains `build-linux-cuda`.
- Checks:
  - Before the commit, the job's shell steps run in a scratch directory outside the tracked tree on a Linux x86-64 host, publishing nothing; WSL2 is enough. The build succeeds, the stub smoke-load reports CUDA, and `ldd` resolves every package library inside the package. A container with no driver refuses the load on `libcuda.so.1`. The step's record in the plan's repository copy notes the host, the build time, and the architecture list ggml chose.
  - Optionally, a fork dispatch of the branch exercises both hosted jobs and is cancelled while the self-hosted Windows CUDA row waits, so nothing publishes. On 2026-09-30 both new builds compiled on GitHub-hosted runners this way, the Linux CUDA job in 82 minutes.
  - The first push run on `master` skips the job.
- Dry run, 2026-09-28, in WSL2 Ubuntu 24.04 on an i9-14900KF (32 threads, 31 GB), with gcc 13.3.0, CMake 3.28.3, and nvcc 12.8.93; tag `b4938` resolves to whisper.cpp `371b5a75` (ggml 0.20.2):
  - Every `run:` block of the job exits 0. `cmake --build` takes about 470 s at `-j32` (12,244 CPU-seconds); a 4-vCPU hosted runner should take 50 minutes or more, which was not measured. The build passes `--parallel "$(nproc)"`, because a bare `--parallel` starts every nvcc compile at once.
  - ggml chose `50-virtual;61-virtual;70-virtual;75-virtual;80-virtual;86-real;89-real;90-virtual;120a-real`, its fixed list for CUDA 12.8 without a native GPU, so a GPU-less runner gets the same list. Against the stub driver, `whisper_print_system_info()` reports `CUDA : ARCHS = 500,610,700,750,800,860,890,900,1200`.
  - `ldd` resolves every ggml library and `libcudart.so.12`, `libcublas.so.12`, and `libcublasLt.so.12` inside the package; only glibc, `libstdc++`, `libgcc_s`, `libgomp`, and the driver's `libcuda.so.1` come from the system. The archive is 743 MB, and it leaves out `libparakeet.so`, which `libwhisper` does not need, as the CPU archives' filter does.
  - Deliberately broken packages fail the job's checks: a CPU-only build fails the CUDA check, and a missing CUDA runtime library or a stray system library fails the `ldd` check.
  - In an `ubuntu:24.04` container with no GPU and no `libcuda`, the load fails with `libcuda.so.1: cannot open shared object file: No such file or directory`; the same container with the stub mounted loads the library and reports CUDA.

### Step 4: Add the `[stt] whisper_backend` setting [completed]

- `crates/gateway/config/src/config/stt.rs` gains `WhisperBackend`: `Auto` (the default), `Cpu`, and `Cuda`, with kebab-case serde, `#[non_exhaustive]`, and `is_auto()`, shaped like `LlamaBackend` in `crates/gateway/config/src/config.rs`.
- `SttPipelineConfig` and `RawSttPipelineConfig` gain `whisper_backend` with `#[serde(default, skip_serializing_if = "WhisperBackend::is_auto")]`, carried through both `Default` impls, `TryFrom<RawSttPipelineConfig>`, and `From<&SttPipelineConfig>`.
- `SttPipelineConfig::whisper_backend()` returns it, documented as consulted only on Windows x86-64 and Linux x86-64.
- `WhisperBackend` is re-exported from `crates/gateway/config/src/config.rs` and `crates/gateway/config/src/lib.rs`.
- Tests, in `crates/gateway/config/src/config/tests/`:
  - `validation.rs`, beside the `llama_backend` tests: each value parses, and `vulkan` is rejected with an error chain that names it and `auto`, `cpu`, and `cuda`.
  - `schema.rs`: `canonical_stt_section_parses_into_the_runtime_shape` asserts `auto`.
  - `serialize.rs`: the `FULL` fixture's `[stt]` carries `whisper_backend = "cuda"`, `enums_round_trip_with_their_toml_spellings` checks the three spellings, and `canonical_stt_input_round_trips_as_canonical_stt` asserts the key is absent under `auto`.

### Step 5: Select and provision the whisper build by backend [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - `WhisperAsset` gains `backend: Option<WhisperBackend>`, documented like `ServerAsset`'s: `Some(Cuda)` on `windows-x86_64-cuda`, `Some(Cpu)` on `linux-x86_64`, and `None` on the macOS and linux-aarch64 rows.
  - `WHISPER_ASSETS` gains `windows-x86_64` (`whisper-b4938-windows-x86_64.zip`, `whisper.dll`, `Some(Cpu)`) and `linux-x86_64-cuda` (`whisper-b4938-linux-x86_64-cuda.zip`, `libwhisper.so`, `Some(Cuda)`), with URLs under `whisper-lib-b4938` and all-zero digests.
  - `whisper_asset(os, arch, backend, gpus)` mirrors `server_asset`. On Windows x86-64 and Linux x86-64, `Auto` becomes `Cuda` when `gpus` reports an NVIDIA GPU and `Cpu` otherwise, and an explicit value selects its own row. Every other platform matches its `None` row.
- In `crates/gateway/local/src/artifacts.rs`, `ArtifactStore::provision_whisper_library(backend, activity)` runs `nvidia_probe()` only for `Auto` on those two platforms, then installs through `whisper_install_asset` and `provision_install` as before. The probe's doc covers both callers' fallbacks.
- In `crates/gateway/stt/api/src/artifacts.rs`, `prepare()` passes `config.stt()`'s `whisper_backend()`, or `auto` when `[stt]` is absent, and logs `provisioned whisper library` with the path, as `crates/gateway/local/src/runtime.rs` logs `provisioned llama-server`.
- Docs:
  - `gateway.local.example.toml`: a commented `whisper_backend` line in `[stt]` with the three values.
  - `guide/src/gateway/05-speech.md`, under "The runtime": the two builds on Windows x86-64 and Linux x86-64, and how the setting chooses between them.
  - `guide/src/gateway/04-local-models.md`, under "The cache directory": the `whisper.cpp/` directory, with one directory per pinned build, such as `b4938-windows-x86_64` and `b4938-linux-x86_64-cuda`.
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
  - `crates/gateway/app/README.md`: the `whisper_backend` row in the `[stt]` table, matching the `[local]` table's `llama_backend` row, and the speech-runtime sentence under "Feature flags".
- Tests:
  - In `assets.rs`, beside the `server_asset` tests: on both platforms an NVIDIA GPU selects the CUDA row, and `None` or an empty probe selects the CPU row; `Cpu` and `Cuda` select their rows whatever the probe reported; every other platform returns its single row under every setting; an unsupported platform is an error.
  - `whisper_assets_cover_the_five_release_platforms` is renamed and covers all seven rows, each a zip with a 64-hex digest under `whisper-lib-b4938`.
  - In `crates/gateway/local/src/artifacts/tests.rs`, `provision_whisper_library_reuses_a_verified_install` and `whisper_installs_never_fall_back_to_an_older_abi` pass an explicit backend, so no test probes the host's GPUs.

### Step 6: Give the native speech fixtures an explicit whisper backend [completed]

- `crates/gateway/stt/api/src/test_fixtures/native.rs` gains a public `fixture_whisper_backend()` beside its `require_fixture` re-export. It returns the `[stt] whisper_backend` spelling from the test-only `PROMPTFORGE_WHISPER_BACKEND`, and `cpu` when that is unset. It returns the spelling rather than a `WhisperBackend`, so an unknown value fails the fixture's config parse with the error that names the accepted values.
- The three native fixture configs write that spelling into `[stt]`:
  - `fixture_service_with_models_on_dedicated_thread` in `crates/gateway/stt/api/tests/common/mod.rs`;
  - `verbose_round_trip_accepts_literal_timestamp_granularities_field` in `crates/gateway/stt/api/src/batch-native-tests.rs`, whose config gains an `[stt]` section;
  - `native_speech_service` in `crates/gateway/app/tests/it/realtime_stt.rs`.
- In `.github/workflows/stt-miri.yml`, the `native-whisper` job's `Provision pinned native fixtures` step adds `PROMPTFORGE_WHISPER_BACKEND=cuda` to `$env:GITHUB_ENV` beside the other fixture variables, so the self-hosted Windows CUDA runner keeps testing the CUDA build.
- Checks, with the `PROMPTFORGE_WHISPER_*` fixture variables set as `stt-miri.yml` sets them:
  - Before the change, on this host in WSL with `nvidia-smi` on PATH, the ignored native suites fail, because `auto` selects the unpublished, all-zero `linux-x86_64-cuda` row.
  - After it, without `PROMPTFORGE_WHISPER_BACKEND`, they pass on the pinned `linux-x86_64` CPU build: `cargo test --locked -p gateway-stt --lib -- --ignored --test-threads=1`, the same with `--test it`, and `cargo test --locked -p gateway --test it realtime_stt::realtime_stt_native_incremental -- --ignored --test-threads=1`.
  - Until the post-merge commit pins the Windows CPU row, a Windows run of those suites sets `PROMPTFORGE_WHISPER_BACKEND=cuda`, as CI does.
  - CI's `native-whisper` job skips fork pull requests, so it first runs after merge. It passes on the CUDA build.

### Step 7: Gate whisper selection on the host's driver and CPU [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - A new `NvidiaProbe` holds the probe's answer: each GPU's compute capability, and the driver version's major number, `None` when it cannot be read.
  - `WhisperAsset` gains `min_driver_major: Option<u64>`: `Some(570)` on `linux-x86_64-cuda`, the CUDA 12.8 floor, and `None` on every other row, `windows-x86_64-cuda` included.
  - `auto_whisper_backend` picks `Cuda` only when the probe reports a GPU and the platform's CUDA row has no floor or a floor at or below the driver's major number. An unreadable version counts as below any floor.
  - `X86_BASELINE` lists the extensions whisper.cpp `371b5a75` enables under `GGML_NATIVE=OFF`: `sse4.2`, `avx`, `avx2`, `bmi2`, `fma`, and `f16c`. They come from the `INS_ENB` options in `ggml/CMakeLists.txt` and the x86 flags in `ggml/src/ggml-cpu/CMakeLists.txt`, where MSVC's `/arch:AVX2` implies FMA and F16C. The list is tied to `WHISPER_RELEASE`.
  - `whisper_asset(os, arch, backend, gpus, x86_extensions)` takes the probe's answer as `Option<&NvidiaProbe>` and the host's detected baseline extensions. On `x86_64`, under every setting, a missing baseline extension returns `LocalError::UnsupportedCpu` in place of the row; other architectures skip the check.
  - `whisper_asset_with_probe` passes both through, and still probes only for `auto` on Windows x86-64 and Linux x86-64.
- In `crates/gateway/local/src/artifacts.rs`:
  - `nvidia_compute_caps()` becomes `nvidia_probe()`: one `nvidia-smi --query-gpu=compute_cap,driver_version --format=csv,noheader` run, still without a console window on Windows, whose output a new pure `parse_nvidia_probe(stdout)` reads.
  - `provision_llama_server_with_cancellation` hands `server_asset` only the probe's compute capabilities, so `server_asset`, `auto_backend`, and their tests stay as they are.
  - A new `host_x86_extensions()` returns the `X86_BASELINE` members `std::arch::is_x86_feature_detected!` reports, under `#[cfg(target_arch = "x86_64")]`, and none elsewhere.
  - `ArtifactStore::provision_whisper_library` passes `nvidia_probe` and `host_x86_extensions()` to selection, and its `# Errors` section names the new variant.
- In `crates/gateway/local/src/error.rs`, `LocalError::UnsupportedCpu { platform, required, missing }` names the selected build, the required extensions, and the missing ones. `SpeechError::WhisperLibrary` already reports it as the provisioning-stage failure.
- Docs state the Linux CUDA build's driver floor, the `auto` fallback below it, that an explicit `cuda` below it is honored and can end the gateway, and the x86 baseline with the failed speech load below it:
  - the `whisper_backend` row of the `[stt]` table in `crates/gateway/app/README.md`;
  - `guide/src/gateway/05-speech.md`, under "The runtime";
  - the commented `whisper_backend` lines in `gateway.local.example.toml`;
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
- Tests:
  - In `assets.rs`, the whisper selection tests move to the new signature with the full baseline, and new ones join them:
    - under `auto` on Linux x86-64, driver 569 or an unreadable version selects `linux-x86_64`, and 570 or later selects `linux-x86_64-cuda`, while Windows x86-64 selects `windows-x86_64-cuda` at any driver version;
    - dropping any one baseline extension fails every x86-64 row under every setting with `UnsupportedCpu` naming it, and the aarch64 rows select as before with no extensions.
  - In `crates/gateway/local/src/artifacts/tests.rs`, `parse_nvidia_probe` reads `compute_cap, driver_version` lines, this host's recorded output among them, and answers `None` for no GPU. `provision_whisper_library_reuses_a_verified_install` and `whisper_installs_never_fall_back_to_an_older_abi` pass the full baseline to `whisper_asset`.
  - llama-server's selection tests pass unchanged.
- Checks:
  - Before the commit, the query runs on this host under Windows and under WSL, and its lines become parser cases.
  - `cargo test --locked -p gateway-local --lib artifacts::` and `cargo test --locked -p gateway-stt --all-features --lib artifacts::tests::` pass.
  - With Step 6's fixtures, the ignored native suites still pass on this host's CPU build, which runs `host_x86_extensions()` on a real CPU. `provision_whisper_library_reuses_a_verified_install` now runs that check too, so on an x86-64 host below the baseline it fails with the named error.

### Step 8: Leave the two new pins for after merge [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`, the `windows-x86_64` and `linux-x86_64-cuda` rows keep their all-zero digests and gain the placeholder comment that `WINDOWS_X86_64_CUDA_BLACKWELL` carried from `66f8f95f` until `56feb2dd` pinned it. The comment says the pin is filled in once the `whisper-lib-b4938` release holds the archive, and that until then the row is fail-closed: the pin can never match, so the download is refused rather than trusted.
- Nothing else in the code changes. The plan's re-export, which moves the pins after merge, rides in this step's commit.
- Checks: the seven-row coverage test and the selection tests pass unchanged, because the comments change no behavior. As the last step of its component, it runs the full suite.

### Step 9: Require the CUDA runtime DLLs in the Windows CUDA package

- In `.github/workflows/whisper-lib.yml`, the `windows-x86_64-cuda` branch of `Package Windows runtime` checks, after it copies the CUDA runtime DLLs, that the bundle holds a `cudart64_*.dll`, a `cublas64_*.dll`, and a `cublasLt64_*.dll`. A missing one throws `<pattern> missing from the runtime bundle`, the message the step's `ggml*.dll` check already uses.
- The copy still searches the toolkit with `-ErrorAction SilentlyContinue`, so the check is what turns a missing DLL into a failed row instead of a bundle without it.
- The CPU row never enters that branch, so its package step is unchanged, and the existing `whisper.dll` and `ggml*.dll` checks stay.
- The edit already sits uncommitted in the working tree. This step verifies it and commits it.
- Checks, in scratch on this Windows host, replaying the step with Step 2's dry-run harness:
  - against a stub toolkit holding all three DLLs, the CUDA row's package step bundles them and passes;
  - with each DLL removed from the stub in turn, the step fails naming that pattern;
  - the CPU row's package step bundles exactly what it did before;
  - the CUDA branch's copy and check also pass against this host's own CUDA toolkit at `CUDA_PATH`.
- It changes no published archive, because the add-only publish never replaces one.
