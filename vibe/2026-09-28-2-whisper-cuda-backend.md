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
  End-to-end testing of the built archives found defects this work introduces, and the last
  steps fix them: auto also checks what each CUDA build needs from the machine, and a graceful
  stop aborts running decodes and cancels speech downloads.
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
  - The users are gateway operators on Windows x86-64 and Linux x86-64. The first is the Linux GPU machine behind wg21.org's Talktron, where recognition sits on the path to a voice turn's first audio.
  - The pinned linux-x86_64 whisper.cpp runtime is the CPU build. On a 32-thread CPU, the final `small.en` pass over a 9.4 s answer takes 2.14 s, and the same sources built with CUDA take 0.13 s.
  - The only Windows x86-64 build is the CUDA build, and its CUDA backend links the NVIDIA driver library. A Windows machine without an NVIDIA driver cannot load it, so it has no speech.
  - Every platform has exactly one pinned whisper build, chosen by OS and architecture alone. Nothing looks at the machine's GPU.
  - Local testing of the new builds on the operator's machines found two process-level defects:
    - the Windows CPU build ends the process when `whisper.dll` is unloaded after a CPU transcription;
    - a gateway on the Linux CUDA build prints CUDA-error aborts at every graceful stop, because speech frees GPU memory after the CUDA runtime has shut down.
  - End-to-end testing on 2026-10-01 and 2026-10-02 found more defects in this work. It ran the archives a fork dispatch built (CI run 36900875610) on the operator's Windows machine and its WSL2 Ubuntu 24.04, with two RTX 3090 GPUs on driver 591.86:
    - On Windows, a graceful stop crashes the gateway with `0xC0000005` once the published CUDA build has decoded on the CPU. That build falls back to the CPU when CUDA finds no usable device, as under a driver older than CUDA 13's 580, and it keeps OpenMP, so Step 12's retirement unloads it under a live OpenMP thread team. The gateway built at Step 11 stopped cleanly in the same runs.
    - On Windows, `auto` gives the CUDA build to GPUs it has no native code for. Its native code covers compute capability 8.6, 8.9, 12.0, and 12.1. GPUs at 7.5, 8.0, and 9.0 get PTX from CUDA 13.3, which a driver with an older CUDA cannot compile, and ggml then ends the process at the first transcription.
    - On Linux, the CUDA build needs `GLIBCXX_3.4.30`, a GCC 12 or later C++ runtime, where the CPU build needs `GLIBCXX_3.4.29`. RHEL 9, its rebuilds, and Amazon Linux 2023 cannot load it, so on those machines with an NVIDIA GPU, `auto` replaces a working CPU build with a failed speech load.
    - `auto` reads GPUs from `nvidia-smi`, which ignores `CUDA_VISIBLE_DEVICES`, so a machine that hides every GPU from CUDA still gets the CUDA build. On Windows that build then decodes on the CPU and crashes at a graceful stop.
    - On the Linux CUDA build, a graceful stop with long decodes running can still end in a CUDA error or a segmentation fault. A running decode cannot be interrupted, so admitted decodes that outlast the retirement deadline leave the retirement abandoned, and the process exits mid-decode.
    - A stop during the 743 MB Linux CUDA download takes 10 s and abandons both the boot command and the speech retirement, because the whisper download ignores the boot command's cancellation token.
- Goals:
  - A CUDA build for linux-x86_64 and a CPU build for windows-x86_64, from the same whisper.cpp tag, `b4938`, built and published by the existing whisper build workflow and pinned by digest like every other runtime. The pins land in one commit after this work merges.
  - One setting, `[stt] whisper_backend`, chooses the build on Windows x86-64 and Linux x86-64 the way `[local] llama_backend` chooses llama-server: `auto` detects an NVIDIA GPU, and an explicit value forces a build.
  - The C ABI, the model files, and decoding stay as they are. The FFI crate changes only to hand a decode's abort flag to whisper.
  - Under the default `auto`, a machine the selection cannot serve gets the CPU build or a failed speech load, never an ended gateway process.
  - Under `auto`, a machine whose GPU the platform's CUDA build cannot use gets the CPU build: on Windows, a driver older than 580, a GPU without native code in the build, or GPUs hidden from CUDA; on Linux, a C++ runtime without the build's `GLIBCXX` version, or GPUs hidden from CUDA.
  - A graceful stop aborts running decodes and cancels speech downloads, so speech retires within the existing deadline.
  - The native speech fixtures name their build, so no test depends on the machine's GPU.
  - A graceful stop retires speech before the process exits, so no speech thread frees native state after the CUDA runtime's teardown.
  - The Windows CPU build survives being unloaded, as the other builds do.
- Non-goals:
  - Replacing or rebuilding the five archives already in `whisper-lib-b4938`.
  - A CUDA build for linux-aarch64, a Vulkan whisper build, or runtime backend loading inside ggml.
  - An operator path override for the whisper library.
  - Any change to llama-server selection.
  - A config UI control for the setting.
  - Running whisper out of process.
  - Older debt this work touches but did not introduce: the MSVC runtime the Windows archives import without bundling it, `speech.gpu` reporting how a build was compiled, and the test-only eager gateway constructor. The Windows CUDA build's driver floor, once listed here, is in scope since end-to-end testing tied it to a defect this work introduces.
  - Rebuilding either CUDA archive: the Windows one without OpenMP, or the Linux one against an older C++ runtime.
  - Issues the end-to-end testing found that predate this work: the gateway log dropping a failed command's cause, the Windows native fixtures' leftover temporary caches, the batch route's status for a full worker queue, the guide not listing the Linux system libraries the archives and the gateway load (libgomp, CA certificates), and SIGTERM and Ctrl-Break skipping the graceful stop.
- Success criteria:
  - The criteria that load a new build hold once the post-merge commit pins the two new rows. Until then those rows fail closed, as the Functional Specification states.
  - A Linux x86-64 machine with an NVIDIA GPU and driver 570 or later, on `auto`, downloads, verifies, and loads the CUDA build: the boot log's library path names it, and `/admin/status` reports `speech.gpu` true. The final `small.en` pass takes about 0.13 s.
  - A Linux x86-64 machine with an older or unreadable driver, or without an NVIDIA GPU, gets the CPU build under `auto`, as today.
  - A Windows x86-64 machine with an NVIDIA GPU on driver 580 or later, every GPU at compute capability 8.6, 8.9, 12.0, or 12.1, keeps the CUDA build under `auto`. A machine without an NVIDIA GPU, on an older driver, or with any other GPU, whose CPU has the x86 baseline, loads the CPU build and transcribes.
  - On Linux, `auto` takes the CUDA build only where the machine's `libstdc++.so.6` defines `GLIBCXX_3.4.30`, and the CPU build elsewhere.
  - With `CUDA_VISIBLE_DEVICES` hiding every GPU by index or by an invalid first entry, `auto` takes the CPU build on both platforms, and a Windows gateway so configured exits 0 at a graceful stop after transcriptions. On Linux a `GPU-` or `MIG-` identifier counts as visible, because the probe reads no UUIDs; on Windows any first entry that is not a device index, an identifier included, counts as hiding every GPU, and an explicit `cuda` restores GPU decoding for a valid identifier.
  - A machine whose CPU lacks a baseline extension fails the speech load with an error naming the required and missing extensions, and the gateway keeps serving.
  - `cpu` and `cuda` force their build on both platforms.
  - The native fixtures pass on a Linux machine with an NVIDIA GPU without extra setup, and CI's native job still exercises the CUDA build.
  - A gateway on the Linux CUDA build stops through `POST /shutdown` or SIGINT with no CUDA error and exit status 0, within the existing shutdown bounds.
  - That holds with six 11-minute batch requests in flight: the stop exits 0 with no CUDA error and no `speech did not retire` warning.
  - A stop during a whisper download ends the boot command with no `did not stop` or `did not retire` warning.
  - On the Windows CPU build, the Windows native suites pass, including unloading the library after a CPU transcription.
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
  - whisper runs in the gateway process, and ggml ends the process with `abort()` on a CUDA error, so `auto` must never select a CUDA build for a machine whose driver cannot run it.
  - Every x86 whisper build is compiled with ggml's fixed non-native baseline, which includes AVX2. Running one on a CPU without that baseline raises an illegal-instruction fault that ends the gateway process.
- Open questions:
  - None that block. Both Linux archives need glibc 2.34, read from their symbol versions, not the 2.35 first recorded here. Their C++ runtimes differ: the CPU archive needs `GLIBCXX_3.4.29` and the CUDA archive `GLIBCXX_3.4.30`.

## Functional Specification

- Actors and workflows:
  - A gateway operator leaves `[stt] whisper_backend` at `auto`, or sets `cpu` or `cuda`, and restarts the gateway.
  - After this work merges, anyone with write access to `cppalliance/promptforge` dispatches the whisper build workflow on `master` for the tag. One commit then pins the two new rows' digests from the release's `SHA256SUMS`.
- Inputs and outputs:
  - Input: `[stt] whisper_backend` is `auto` (the default), `cpu`, or `cuda`. Serialization omits it when it is `auto`.
  - Machine inputs to selection: the NVIDIA probe's answer, which is each GPU's compute capability and the driver version; whether `CUDA_VISIBLE_DEVICES` hides every GPU; on Linux, whether the machine's `libstdc++.so.6` defines the CUDA build's `GLIBCXX` version; and the CPU's instruction-set extensions.
  - Output: the whisper.cpp library speech loads, or a named selection error that fails the speech load, and one boot log line, `provisioned whisper library`, with the library's path.
    - The install directory in that path names the build, such as `b4938-linux-x86_64-cuda`. The path reaches stdout and so a service's journal, but `gateway.log` redacts local paths.
    - `GET /admin/status` reports `speech.gpu`, true when the loaded build has CUDA, whichever log is read.
  - Workflow output: `whisper-b4938-windows-x86_64.zip` and `whisper-b4938-linux-x86_64-cuda.zip` in the `whisper-lib-b4938` release, with their lines added to its `SHA256SUMS`.
- States and validation:
  - On Windows x86-64 and Linux x86-64:

    | Setting | `nvidia-smi` reports an NVIDIA GPU | No NVIDIA GPU, or the probe fails |
    | --- | --- | --- |
    | `auto` | The CUDA build when the machine meets its requirements below, and the CPU build otherwise. | The CPU build. |
    | `cpu` | The CPU build. | The CPU build. |
    | `cuda` | The CUDA build. | The CUDA build, which fails to load without a driver. |

  - `auto` takes the CUDA build only when every machine requirement of the platform's CUDA build holds, and the CPU build otherwise:
    - the driver meets the build's floor: 570 on Linux (CUDA 12.8) and 580 on Windows (CUDA 13). An unreadable version meets no floor.
    - On Windows, every GPU's compute capability is one the build has native code for: 8.6, 8.9, 12.0, or 12.1.
    - On Linux, the machine's `libstdc++.so.6` defines `GLIBCXX_3.4.30`.
    - `CUDA_VISIBLE_DEVICES` leaves a GPU visible: it is unset, or its first entry is a device index below the GPU count or, on Linux, a `GPU-` or `MIG-` identifier. This follows CUDA's rule that only the devices before the first invalid entry are visible. On Windows an identifier counts as hiding every GPU, because the probe reads no UUIDs to match it and only the Windows CUDA build crashes at a graceful stop after a CPU fallback.
  - On an x86 machine, every setting first requires the builds' shared CPU baseline, and a missing extension fails selection.
  - The probe and the other machine checks run only under `auto`, and only on those two platforms.
  - Every other platform has exactly one build, and every setting selects it. The setting is documented as consulted only on Windows x86-64 and Linux x86-64, as `[local] llama_backend` is documented as consulted only on Windows x86-64.
  - An unknown value is a configuration error that names the rejected value and the accepted ones.
  - Until the post-merge pin commit, the two new rows fail closed:
    - Under `auto`, a Linux x86-64 machine that meets the CUDA build's requirements, and a Windows x86-64 machine that does not meet them, select an unpinned row. Their speech load fails at the download or the digest check, and the gateway keeps serving.
    - That Windows set includes NVIDIA machines the requirements turn away, such as a Turing GPU or a driver older than 580, which get the pinned CUDA build on `master` today. `cuda` keeps them on it until the pin commit.
    - Meanwhile `cpu` on Linux and `cuda` on Windows select today's pinned builds.
- Errors and recovery:
  - The chosen build downloads when missing and is verified against its pin. A download, verification, or load failure fails the speech load and names its stage, and the gateway never switches builds on its own.
  - Under `auto`, a machine that fails a requirement gets the CPU build, so it never reaches a CUDA build it cannot run.
  - An explicit `cuda` is honored on both platforms whatever the machine, and each failed requirement has its own outcome. The docs name every requirement, with `auto` or `cpu` as the recovery.
    - On Linux below the floor, it can end the gateway at the first transcription on a GPU the build has no native code for.
    - On Windows, a GPU without native code under a driver older than CUDA 13.3's ends the gateway at the first transcription.
    - On Windows, where CUDA finds no usable device, the build decodes on the CPU, and a graceful stop then ends the gateway.
    - On Linux without the C++ runtime, the build fails to load.
  - A CPU missing a baseline extension fails the speech load with an error naming the required and missing extensions. Speech stays unavailable and the gateway keeps serving; no setting recovers it, because every x86 build shares the baseline.
  - As today, a failed speech load never stops the gateway and is never retried in-process. A restart is the recovery.
  - A graceful stop retires speech after the command worker stops, within the command worker's `WORKER_JOIN_TIMEOUT` deadline. Admission closes, which ends Realtime sessions and aborts every running decode after its current encoder pass or decoder step. The speech workers then join. Past that deadline the retirement is abandoned with a warning, as a stuck command already is.
  - A request whose decode is aborted fails, as admitted requests already do at shutdown.
  - A stop during speech provisioning cancels the whisper library and speech model downloads between chunks. An extraction already under way finishes.
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
  - `gateway-local` owns the GPU probe, reused from llama-server, and row selection with its machine-capability checks (the driver floor, the native-code list, the C++ runtime, CUDA's device visibility, and the CPU baseline), and provisioning.
  - `gateway-whisper-ffi` and `gateway-stt-backend-whisper` change only to hand a decode's abort flag to whisper's `abort_callback`. `gateway-stt-engine` carries the flag on each decode request, and `gateway-stt` sets it from the runtime's admission epoch.
  - The native speech fixtures and CI's native job name their backend explicitly.
  - `gateway`'s `Gateway::serve` retires speech at a graceful stop, through the existing `SpeechService::shutdown`.
- Modules and interfaces:
  - The Windows CPU build is a `windows-x86_64` matrix row on a GitHub-hosted Windows runner, and it builds on push like the other rows.
    - It configures like the Windows CUDA row without `-DGGML_CUDA=ON` and with `-DGGML_OPENMP=OFF`, packages like it without the CUDA runtime DLLs, and smoke-loads the same way.
    - With OpenMP off, ggml runs its own thread pool, and the build no longer imports the MSVC OpenMP runtime, `VCOMP140.DLL`, whose unload under a live thread team ended the process. Master's Windows llama-server build sets the same flag in `crates/build-llama-cuda/src/cmake.rs`.
    - The shared `Package Windows runtime` step fails the CUDA row when its bundle lacks a `cudart64_*.dll`, `cublas64_*.dll`, or `cublasLt64_*.dll`, the Windows counterpart of the Linux CUDA job's `ldd` check. Master's step copied them with `-ErrorAction SilentlyContinue` and never checked, so a toolkit missing one produced a bundle without it.
  - The Linux CUDA build is a job of its own in the same workflow, and it runs only on `workflow_dispatch`, so push-triggered runs skip it.
    - Runner and toolchain: GitHub-hosted `ubuntu-22.04` with no GPU and no driver, and NVIDIA's CUDA 12.8 apt build components: the compiler, cudart, cuBLAS, and the driver stubs. Its timeout is sized like the GitHub-hosted Blackwell CUDA build's.
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
    - `WhisperAsset` also carries an optional minimum driver version: 570 on `linux-x86_64-cuda`, the CUDA 12.8 floor, and 580 on `windows-x86_64-cuda`, the CUDA 13 floor.
  - Selection mirrors `server_asset`: `whisper_asset(os, arch, backend, gpus)` picks the row, and `whisper_asset_with_probe` runs the probe only for `auto` where both builds exist.
    - On the two platforms, `auto` becomes `Cuda` when the probe reports an NVIDIA GPU that meets the row's driver floor, and `Cpu` otherwise, including when the driver version cannot be read. An explicit value selects its own row.
    - Every other platform matches its single row.
    - Before any x86 row is returned, under every setting, the machine must support every extension the pinned `GGML_NATIVE=OFF` baseline enables. The list is read from whisper.cpp `371b5a75`'s `ggml/CMakeLists.txt`, the machine's side comes from `std::arch::is_x86_feature_detected!`, and other architectures skip the check.
    - A missing extension fails selection with a new `LocalError` variant naming the required and missing extensions. `LocalError` is `#[non_exhaustive]`, so the variant is additive.
    - Both checks take their inputs (the probe's answer, the machine's extensions) as arguments, so tests run on any machine.
  - Each CUDA row's machine requirements are data on the row, beside `min_driver_major`. Like `X86_BASELINE`, they are tied to `WHISPER_RELEASE`, and a release bump re-reads them from its archives.
    - `WhisperAsset.native_compute_caps: Option<&[(u64, u64)]>` is `Some(&[(8, 6), (8, 9), (12, 0), (12, 1)])` on `windows-x86_64-cuda` and `None` elsewhere. Those are the capabilities the pinned archive's `ggml-cuda.dll` fatbinary carries native code for; its PTX, ISA 9.3 from CUDA 13.3, covers 7.5, 8.0, and 9.0.
    - `WhisperAsset.min_glibcxx: Option<&str>` is `Some("GLIBCXX_3.4.30")` on `linux-x86_64-cuda` and `None` elsewhere. `libggml-cuda.so.0` needs that version for `std::condition_variable::wait`.
  - `auto_whisper_backend` takes the CUDA row only when all of these hold. Every input stays an argument, so tests run on any machine.
    - The probe reports a GPU, and `CUDA_VISIBLE_DEVICES` does not hide every GPU.
    - The driver meets the row's floor.
    - When the row lists native capabilities, every probed GPU's capability is in the list.
    - When the row names a `min_glibcxx`, the machine's C++ runtime defines it.
  - Two pure helpers sit in `assets.rs`:
    - `cuda_visible_devices_hides_every_gpu(value: Option<&str>, gpu_count: usize) -> bool` applies CUDA's rule that only the devices before the first invalid entry are visible.
    - `libstdcxx_defines(library: &[u8], version: &str) -> bool` finds the version name, NUL-terminated, in the library's bytes.
  - `ArtifactStore::provision_whisper_library_with_cancellation` gathers the two machine readings beside `nvidia_probe`, only under `auto` where both builds exist, and only after the probe reports a GPU. They travel to selection as a `CudaMachine`.
    - It reads `CUDA_VISIBLE_DEVICES`.
    - On Linux it reads the first that exists of `/usr/lib/x86_64-linux-gnu/libstdc++.so.6`, `/usr/lib64/libstdc++.so.6`, `/lib/x86_64-linux-gnu/libstdc++.so.6`, `/lib64/libstdc++.so.6`, and `/usr/lib/libstdc++.so.6`, and no library found counts as lacking the version.
    - llama-server's selection reads neither.
  - The NVIDIA probe in `crates/gateway/local/src/artifacts.rs` reads each GPU's compute capability and the driver version in one `nvidia-smi --query-gpu=compute_cap,driver_version --format=csv,noheader` call. llama-server's selection keeps reading only the capabilities, so its behavior is unchanged.
  - `ArtifactStore::provision_whisper_library(backend, activity)`, in `crates/gateway/local/src/artifacts.rs`, still returns the library path, and goes through the existing verified install path once a row is selected.
    - The fixes replace it with `ArtifactStore::provision_whisper_library_with_cancellation(backend, activity, token)`, which passes `token` to `provision_install`. The token-free method is removed, because after `prepare()` moves over no production code calls it; its one test passes `None`.
    - A fired token stops a download between chunks and at phase boundaries, not inside an extraction or the probe, and returns `LocalError::Cancelled`.
  - `prepare()`, in `crates/gateway/stt/api/src/artifacts.rs`, reads the setting from `[stt]`, passes it to provisioning through `prepare_impl`, which takes the provisioning as an argument, and logs `provisioned whisper library` with the path, as `crates/gateway/local/src/runtime.rs` logs `provisioned llama-server`.
    - It takes the load's `CancellationToken` and passes it to the library download and, through the existing `ensure_model_with_cancellation`, to each speech model download.
    - A `LocalError::Cancelled` from either download becomes `SpeechError::InitialLoadCancelled`, the error `SpeechService::load_initial` already documents for a cancelled load.
    - `GenerationState::load_initial`, in `crates/gateway/stt/api/src/generation.rs`, passes its `cancel`. The boot command's token already reaches `load_initial` through `load_speech` in `crates/gateway/app/src/boot_load.rs`.
  - A running decode aborts when the runtime's admission shuts down:
    - `SessionEpoch`, in `crates/gateway/stt/api/src/admission.rs`, keeps its cancelled flag in a shareable `Arc<AtomicBool>`. `GenerationLease::decode`, in `generation-lease.rs`, is the one funnel every batch and Realtime decode takes, and it attaches that flag to the request.
    - `DecodeRequest`, in `crates/gateway/stt/engine/src/decoder.rs`, carries the flag through a new `with_cancellation` builder and an accessor. The worker passes it on untouched.
    - `transcribe_blocking`, in `crates/gateway/stt/backend-whisper/src/model.rs`, sets it on the pass's `FullParams` through a new `FullParams::set_abort_flag(Arc<AtomicBool>)` in `crates/gateway/stt/whisper-ffi/src/params.rs`.
    - `transcribe_blocking` fails at once when the flag already reads true. whisper first reads the flag only after an encoder pass, which takes seconds per queued `small.en` request on the CPU build, so a decode still queued at the stop would otherwise outlast the deadline.
    - `FullParams::apply` writes a non-panicking `extern "C"` callback that loads the flag with Acquire ordering, and the flag's address as its user data. `raw::FullParams.abort_callback` becomes a nullable function pointer of the same size, so the 304-byte layout test still holds.
    - whisper.cpp b4938 polls the callback after each encoder pass, one per 30 s window, and after each decoder step, never inside a graph computation. The pass then returns -6, -8, or -9, which surfaces as the existing `WhisperError::Inference`.
    - The decode's job is then released, `wait_until_idle` returns, and the speech engine's workers join.
    - A load (`whisper_init`) has no abort, so it keeps its residual risk.
  - The native fixture configs in `crates/gateway/stt/api/tests/common/mod.rs`, `crates/gateway/stt/api/src/batch-native-tests.rs`, and `crates/gateway/app/tests/it/realtime_stt.rs` set `whisper_backend` from a test-only `PROMPTFORGE_WHISPER_BACKEND`, defaulting to `cpu`. It sits beside the existing `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO`, and the `native-whisper` job in `.github/workflows/stt-miri.yml` sets it to `cuda`.
  - `Gateway::serve`, in `crates/gateway/app/src/runner.rs`, keeps a clone of the process's `SpeechService` under the `stt` feature.
    - After the bounded command-worker join, it runs `SpeechService::shutdown` on `spawn_blocking` under a deadline shared with that join.
    - On expiry it logs a warning and abandons the call, as the join abandons a stuck command.
    - The speech engine's signal-and-detach `Drop` does not change. The retirement first closes admission, which aborts running decodes as above.
- File and public API changes:
  - Changed: `.github/workflows/whisper-lib.yml`, and `.github/workflows/stt-miri.yml`'s native job with the three native fixture files; `Gateway::serve` in `crates/gateway/app/src/runner.rs`, `crates/gateway/stt/backend-whisper/tests/native_whisper.rs`, and `crates/gateway/app/tests/it/realtime_stt/authentication.rs`.
  - Changed by the fixes after Step 12:
    - selection and provisioning in `crates/gateway/local/src/artifacts/assets.rs` and `crates/gateway/local/src/artifacts.rs`;
    - `crates/gateway/stt/api/src/artifacts.rs`, `generation.rs`, `generation-lease.rs`, and `admission.rs`, with the `gateway-stt` test fixtures;
    - `crates/gateway/stt/engine/src/decoder.rs`, with the speech engine's test fixtures;
    - `crates/gateway/stt/backend-whisper/src/model.rs`;
    - `crates/gateway/stt/whisper-ffi/src/raw.rs`, `params.rs`, and `context.rs`.
  - New public API: `gateway_config::WhisperBackend`, `SttPipelineConfig::whisper_backend`, and the `LocalError` variant for an unsupported CPU.
    - The fixes add `ArtifactStore::provision_whisper_library_with_cancellation`, `DecodeRequest::with_cancellation` and its accessor, and `FullParams::set_abort_flag`.
  - Changed public API: `ArtifactStore::provision_whisper_library` takes the backend. Its one production caller is `prepare()`. The fixes then replace it with `provision_whisper_library_with_cancellation`, which also takes the token.
  - Docs:
    - `gateway.local.example.toml`: a commented `whisper_backend` line with the three values.
    - `guide/src/gateway/05-speech.md`: the two builds on Windows x86-64 and Linux x86-64, how the setting chooses between them, the Linux CUDA build's driver floor with the `auto` fallback below it, and the x86 CPU baseline. The `[stt]` row in `crates/gateway/app/README.md` and the example config state the floor and the baseline too.
    - `guide/src/gateway/04-local-models.md`: the `whisper.cpp/` cache directory, with one directory per pinned build, such as `b4938-windows-x86_64` and `b4938-linux-x86_64-cuda`.
    - `guide/promptforge-gateway-guide.md`, the assembled guide, carries the same two changes.
    - `crates/gateway/app/README.md`: the `whisper_backend` row in the `[stt]` table, matching the `[local]` table's `llama_backend` row, and the speech-runtime sentence.
    - The fixes extend the same four places, and the assembled guide is regenerated:
      - `auto`'s requirements: the Windows floor and native capabilities, the Linux C++ runtime (a GCC 12 or later `libstdc++`, which RHEL 9 and Amazon Linux 2023 lack), and hidden GPUs.
      - The outcomes of an explicit `cuda` for each failed requirement.
      - The first-use PTX compilation on GPUs without native code.
      - The CUDA builds' cache use, about 1.2 GB on Windows and 1.7 GB on Linux counting the downloaded archive the cache keeps.
  - No config UI change. The Speech card in `crates/gateway/config-ui/ui/src/pages/settings-page.ts` saves the loaded section plus its own edits, so the setting survives a save.
- Data, persistence, failure, security, and privacy constraints:
  - Cache layout: `<cache_dir>/whisper.cpp/<release>-<platform>/`, so a machine's CPU and CUDA installs sit side by side.
  - The probe is one `nvidia-smi` run per speech boot, only under `auto` on the two platforms. On Windows it runs without a console window, as llama-server's probe does.
  - The setting persists only in `gateway.toml`, and it is never written when `auto`.
  - Selection is the only guard against the runtime's process-ending failures, because whisper runs in-process and ggml aborts on a CUDA error. A selection failure reaches the existing provisioning-stage speech error, which leaves speech unavailable and the gateway serving.
  - The machine checks run once per speech boot, only under `auto` where both builds exist. The C++ runtime is read only on Linux and only after the probe found a GPU.
  - The abort flag lives in an `Arc` that the pass's `FullParams` owns, and `WhisperState::full` borrows those params for the whole native call, so the callback's user data outlives every read.

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
  - Machine-capability checks, in `crates/gateway/local`, through injected inputs:
    - under `auto` on Linux x86-64, driver 569 or an unreadable version selects the CPU row, and 570 or later selects the CUDA row. Under `auto` on Windows x86-64, driver 579 or an unreadable version selects the CPU row, and 580 or later selects the CUDA row when every GPU is native. The Windows case replaces `auto_takes_the_windows_cuda_whisper_build_at_any_driver_version`.
    - the probe parser reads `compute_cap, driver_version` lines, and llama-server's existing selection tests still pass;
    - a machine missing any baseline extension fails selection on every x86 row under every setting, with an error naming it; a complete set selects as before, and other architectures skip the check.
    - under `auto` on Windows x86-64 at driver 591, a GPU at 7.5, 8.0, 9.0, or 6.1, alone or beside a native one, selects the CPU row;
    - under `auto` on Linux x86-64 with a GPU on driver 570 or later, a C++ runtime without `GLIBCXX_3.4.30` selects the CPU row and one with it the CUDA row;
    - a `CUDA_VISIBLE_DEVICES` that hides every GPU selects the CPU row on both platforms;
    - `cuda_visible_devices_hides_every_gpu`: unset, `0`, `1,0`, `0,-1`, `GPU-...`, and `MIG-...` leave a GPU visible; empty, `-1`, `2` with two GPUs, and `none` hide them;
    - `libstdcxx_defines` finds a NUL-terminated version, and rejects a longer one that shares its prefix, such as `GLIBCXX_3.4.300`;
    - explicit `cpu` and `cuda` ignore every machine check, and the seven-row coverage test asserts each row's new fields.
  - Provisioning cancellation, in `crates/gateway/local` and `gateway-stt`: a fired token makes `provision_whisper_library_with_cancellation` return `LocalError::Cancelled` without downloading, and `prepare` hands the load's token to the library and model provisioning.
  - Decode abort, with scripted decoders and no GPU:
    - in `gateway-stt`, with a scripted decoder parked until its request's cancellation fires, `SpeechService::shutdown` returns within a second and the decoder saw the flag; without the change the shutdown waits on the park;
    - batch, Realtime interim, and Realtime final decodes all carry the flag;
    - the runner's drain tests that hold a bare worker job still abandon the retirement at the bound.
  - Speech retirement at a graceful stop, in `crates/gateway/app/src/runner.rs` beside `serve_abandons_a_worker_that_ignores_cancellation_after_the_join_bound`, with a scripted speech service and no GPU:
    - when `serve` returns, both scripted decoders report their workers dropped and speech reports not ready;
    - with a worker job held across the stop, `serve` returns within one to two join bounds, and releasing the job then lets the workers drop;
    - the realtime authentication test that served one speech service from two servers gives each server its own.
- Integration and end-to-end:
  - Tests that go through provisioning, the native fixtures included, pass an explicit backend, so no test depends on the machine's GPUs. The existing warm-cache reuse and no-older-ABI tests keep passing under the new signature.
  - On a Linux x86-64 machine with `nvidia-smi` on PATH (WSL here), the ignored native suites pass without `PROMPTFORGE_WHISPER_BACKEND` and use the pinned CPU build: `cargo test --locked -p gateway-stt --lib -- --ignored --test-threads=1`, the same with `--test it`, and the gateway's realtime native test.
  - The Linux CUDA job's shell steps run in a scratch directory on a Linux x86-64 machine before the workflow change lands, publishing nothing. WSL2 is enough.
    - Expect a successful build, a stub smoke-load that reports CUDA, and `ldd` resolving every library inside the package.
    - A container with no driver refuses the load on `libcuda.so.1`.
    - Record the machine, the build time, and the architecture list ggml chose.
  - The Windows CPU row's configure, package, and smoke-load steps run on a Windows machine the same way.
  - The Windows CUDA row's package step, replayed against a stub toolkit, bundles its three CUDA runtime DLLs, and with any one removed from the stub it fails naming it.
  - The Windows CPU row, built on a Windows machine with `-DGGML_OPENMP=OFF`:
    - its smoke-load reports no OpenMP, and `dumpbin /dependents` lists no `VCOMP140.DLL`;
    - a probe that transcribes on the CPU and then unloads the library exits cleanly;
    - the Windows native suites pass on it;
    - a `small.en` transcription is timed against the OpenMP build.
  - On a Linux x86-64 machine with an NVIDIA GPU, a gateway on the Linux CUDA package stops repeatedly, by `POST /shutdown` and by SIGINT, with no CUDA error and exit status 0. The `native_whisper` suite on that package also exits without one.
  - With speech engines that shut down before each test ends, the `native_whisper` suite passes on the Linux CUDA package, the Windows CPU build without OpenMP, and the pinned Windows CUDA build.
  - A gateway stopped after a transcription exits 0 on the Linux CPU build and on the pinned Windows CUDA build.
  - Optionally, a dispatch of the branch's workflow on a fork exercises both GitHub-hosted jobs. The self-hosted Windows CUDA row stays queued there, so the publish never runs, and the run is cancelled. On 2026-09-30 both new builds compiled on GitHub-hosted runners this way.
  - Before landing, the add-only publish runs against a throwaway release on a fork, which needs the operator's go-ahead because it creates and deletes a public release. An archive the release already holds stays byte-identical, a new one is uploaded, `SHA256SUMS` keeps its old lines verbatim and gains the new one, and a re-run uploads nothing.
  - After merge, the release dispatch leaves the five existing archives and their lines unchanged, and the pin commit's pins equal the new lines in `SHA256SUMS`.
  - After merge, the first push run on `master` builds the Windows CPU row and skips the Linux CUDA job, and CI's `native-whisper` job passes on the CUDA build.
  - Optionally, before the pin commit merges, a gateway built from it smoke-tests each new archive on real hardware, while the selection unit tests cover `auto`'s choice between builds. A bad archive found this way is withdrawn before any merged pin names it.
    - The Linux CUDA build, on a Linux machine with an NVIDIA GPU on driver 570 or later: under `auto` it loads, the log path names `b4938-linux-x86_64-cuda`, `/admin/status` reports `speech.gpu` true, and the final `small.en` pass takes about 0.13 s.
    - The Windows CPU build, on a Windows machine with `whisper_backend = "cpu"`: it loads and transcribes.
  - The ignored `native_whisper` suite in `gateway-stt-backend-whisper` gains an abort test. A final decode of the fixture audio, repeated to several minutes, fails within a second of its flag being set mid-pass; a decode whose flag is already set fails at once; and the clip with its flag unset transcribes. It runs on the Linux CUDA package, the pinned Linux CPU build, the Windows CPU build, and the pinned Windows CUDA build.
  - The fixes are checked with the end-to-end round's scratch scripts in `vibe/scratch/run-36900875610/scripts/`. They run on scratch clones of each step's commit, whose two new rows point at run 36900875610's archives served from loopback:
    - Windows, `win_gateway.py hidden`: under `auto` with `CUDA_VISIBLE_DEVICES=-1` the gateway takes the CPU build, and every stop exits 0.
    - Windows, `win_gateway.py boots`: on this machine (RTX 3090 at 8.6, driver 591) `auto` still takes the CUDA build.
    - Linux, `wsl-longaudio.sh`: ten stops, by `POST /shutdown` and SIGINT, with six 11-minute requests in flight exit 0 with no CUDA error and no `speech did not retire` warning. `win_gateway.py long` shows the same on both Windows builds.
    - Linux, `wsl-lifecycle2.sh` with `ONLY=download`: the mid-download stop exits within a second with no abandonment warning, and the next boot resumes the partial download.
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
  - On this machine, cargo runs in WSL through the Project survey's wrapper, and `workshop-gateway` runs with `--test-threads=1`, because without nextest its fixture tests race into a pre-existing ETXTBSY under a threaded runner. The WSL toolchain has no clippy, so clippy runs with the Windows toolchain for the changed crates, none of which builds the config UI.

## Decision Record

- Decisions:
  - Follow master's precedents rather than invent new policy. The user asked to "reimplement the same logic" and said "I want to follow the precedents already in master, not invent one."
  - One workflow. The whisper build workflow already builds CPU, CUDA, and Metal variants side by side into one release per tag, so the two new builds join it, and every future tag bump stays one dispatch and one release.
  - Add-only publishing is the smallest change that lets an already-published tag gain a row without replacing the archives shipped gateways pin. It also protects those pins from any accidental re-dispatch.
  - The Linux CUDA build compiles only on a release dispatch. The user asked for it ("Can we somehow make this Linux CUDA build optional on actual release only?"), and master already made its other GitHub-hosted CUDA compile, the Blackwell llama-server build, manual-only.
  - A Windows CPU build in this work. The user asked for it ("Windows CPU only would be beneficial in this PR too."), and it gives a Windows machine without an NVIDIA GPU working speech.
  - `auto` detects the GPU the way `[local] llama_backend = "auto"` does, reusing its `nvidia-smi` probe, and downloads the build it picks. On Linux it also requires the CUDA build's driver floor, 570, because whisper runs in-process and a CUDA error there ends the gateway, where llama-server fails as a child process.
    - Machines with sm_86 or sm_89 GPUs on drivers 525 to 569, which could run the build, get the CPU build under `auto`, and `cuda` remains their override.
  - An explicit `cuda` is honored as given. Below the Linux floor it can end the gateway at the first transcription on a GPU without native code in the build, which the docs state.
  - Selection checks the x86 CPU baseline under every setting and fails the speech load with a named error, so a machine below the baseline loses speech rather than the gateway.
    - It covers every x86 row because they share the baseline, which also closes the same exposure in the older Linux CPU and Windows CUDA rows.
  - The native fixtures read a test-only backend variable that defaults to `cpu`, and CI's native job sets `cuda`, following the existing `PROMPTFORGE_WHISPER_*` fixture variables.
  - The machine-capability and fixture fixes join this plan ahead of the pins, in the user's words: "Fold these fixes into the whisper plan ahead of its checksum step." The pins land last, after merge, because they make the fixed paths reachable.
  - One pull request, #91, merges before the release, and one commit after merge pins both new rows. The user asked for it: "When the PR gets merged into upstream master after review, anyone can run the workflow, update the checksum lists in a single commit."
    - It follows master's Blackwell llama-server row. `66f8f95f` added that row with an all-zero digest and a placeholder comment, and after the first release run `56feb2dd` pinned it in one commit.
    - It replaces the dispatch from the pull request's head, which needed that head pushed to `cppalliance/promptforge` and published archives from workflow changes no reviewer had approved. That dispatch had replaced the earlier two-PR landing.
    - Consequence: until the pin commit, `master`'s `auto` sends a Linux NVIDIA machine on driver 570 or later, and a Windows machine whose probe finds no NVIDIA GPU, to an unpinned row whose speech load fails. `cpu` on Linux and `cuda` on Windows keep today's builds. The Blackwell row had the same window.
    - The machine requirements the fixes add widen the Windows side of that window. A Windows NVIDIA machine they turn away, such as one with a Turing GPU or a driver older than 580, also reaches the unpinned CPU row until the pin commit, and `cuda` keeps it on the pinned CUDA build.
  - The setting lives in `[stt]`, not `[local]`. It is a speech concern read at speech boot, and `[local]` configures llama-server and the cache.
  - The CUDA runtime ships inside the Linux archive. cudart, cuBLAS, and cuBLASLt ride along, as the Windows CUDA archive's DLLs do, so a machine needs a driver and no toolkit.
  - Each library is packaged once, under its soname. Copying every symlink name, as the CPU archive does, would triple the 170 MB CUDA backend.
  - ggml's default CUDA architecture list, as for the Windows CUDA row. It carries native code for sm_86, sm_89, and sm_120, and PTX for the rest.
  - CUDA 12.8 for the Linux build. It is the first toolkit with Blackwell's sm_120, and it needs driver 570 or later.
  - The provisioning log names the library path, as llama-server's does. The install directory in the path names the build, so no build-label type is needed.
  - The Windows CUDA package requires its three CUDA runtime DLLs, as the Linux CUDA job's `ldd` check requires its libraries. The user asked to include this working-tree edit "if those changes are positive and doesn't introduce any defects".
  - The Windows CPU row builds without OpenMP, as master's Windows llama-server build does, so unloading the library cannot end the process. The user chose to fix this in this pull request.
    - Its unload check loads once per thread, as the gateway and Rust's built-in test harness do. A reload on the same thread trips ggml's load-time check, so the user chose "option a": keep the flag, probe one load per thread, and record that check as an upstream limitation.
  - A graceful stop retires speech in `Gateway::serve`, which every production path runs, through the existing `SpeechService::shutdown`, so whisper frees its GPU memory while the CUDA runtime is alive. The user chose to fix this in this pull request.
    - It shares the command worker's join deadline, so the documented shutdown bounds stay as they are.
    - It runs after the command worker stops, so a speech load that worker was still running has either published its runtime or been cancelled.
    - A service injected through `Gateway::with_speech_service` is retired when that `serve` stops, so one service serves one `serve` call. Only the gateway's own tests inject one, and one realtime test that reused a service across two servers changes with it.
  - The upload itself enforces add-only, so the guarantee does not rest on the job's filtering alone.
    - New archives go up with `gh release upload` without `--clobber`, which refuses a name the release already holds. A filtering mistake then fails the publish instead of replacing an archive that shipped gateways and `.github/workflows/stt-miri.yml` pin.
    - `SHA256SUMS` is the one asset replaced, and its existing lines stay verbatim.
  - A published new archive is withdrawn by hand, never replaced by the workflow.
    - The new archives come from the post-merge dispatch, and one found bad before the pin commit merges follows this rule.
    - If one fails, a maintainer deletes that archive and its `SHA256SUMS` line, which is safe while no merged pin names it. The next dispatch's add-only publish uploads the fixed rebuild, whose digest the pin commit takes.
    - Once a merged pin names an archive, it never changes.
  - Real-hardware checks of the new archives leave this pull request with the pins, because the archives exist only after the post-merge dispatch. They become the optional smoke test before the pin commit merges, and the withdrawal rule, which the user confirmed with "ok to both", stays.
    - The selection unit tests cover `auto`'s choice, so forcing the Windows CPU build with `cpu` on an NVIDIA machine stands in for a Windows machine without an NVIDIA GPU.
  - The defects the end-to-end testing found in this work are fixed in this pull request, ahead of the pins. The user asked to "fix every found issue that are specific to this PR's change or regression from this PR". Issues the same testing found that predate this work stay out and are tracked apart.
  - Selection stays the guard. Each CUDA row's machine requirements are data on the row, which `auto` checks the way it already checks the driver floor and the x86 baseline.
    - Windows `auto` requires driver 580, CUDA 13's floor, and native code for every GPU.
      - The native list was read from the pinned archive's fatbinary on 2026-10-02. It has native code for sm_86, sm_89, sm_120, and sm_121, and PTX for compute_75, compute_80, and compute_90 at ISA 9.3, which is CUDA 13.3 (cudart's file version is 13030).
      - On this machine's driver 591.86 (CUDA 13.1), forcing the PTX path failed with "the provided PTX was compiled with an unsupported toolchain", and ggml ended the process.
    - The compute-capability match rejected below now applies to the Windows row, because its revisit condition holds: the two CUDA builds' lists differ. The Windows list describes an archive pinned forever, so it cannot drift from the code it gates.
    - Every GPU must be native, not just one, because whisper uses CUDA's device 0, whose fastest-first order need not match `nvidia-smi`'s.
    - The gate closes the Windows stop crash under `auto` for its likely cause, a driver older than 580, and for GPUs hidden from CUDA. Machines it turns away keep `cuda` as their override, as Linux sm_86 and sm_89 GPUs on drivers 525 to 569 do.
    - Linux `auto` requires the CUDA row's `GLIBCXX_3.4.30` in the machine's C++ runtime, so RHEL 9, its rebuilds, and Amazon Linux 2023 keep the CPU build they had before this work. The archive itself does not change, and the alternative of rebuilding it is rejected below.
    - A `CUDA_VISIBLE_DEVICES` that hides every GPU counts as no GPU, by CUDA's documented rule, because `nvidia-smi` ignores the variable. The probe reads no UUIDs, so a first entry that is a `GPU-` or `MIG-` identifier cannot be matched. On Linux it counts as visible unmatched, and a stale identifier then leaves the CUDA build selected, which the docs state. On Windows it counts as hiding every GPU, because the Windows CUDA build's CPU fallback crashes at a graceful stop; an explicit `cuda` restores GPU decoding for a valid identifier.
  - A graceful stop aborts running decodes through whisper.cpp's own `abort_callback`.
    - It is fed by the runtime's admission epoch, the signal that already settles admitted requests at shutdown.
    - The speech engine worker's stopping flag cannot serve, because it is set only after the drain wait.
  - The boot command's token reaches the whisper library and speech model downloads, as it reaches llama-server's.
    - That matches the runner's comment that a quit during provisioning stops the download.
    - The models ride along because the cancellable model download already exists and costs nothing more.
- Rejected alternatives:
  - A separate workflow and release tag for the CUDA build: it duplicates the Linux flags and packaging and splits one tag across two releases. Revisit if a build ever needs a different whisper.cpp tag than the other rows.
  - Re-dispatching the workflow as it is: it would replace the five pinned archives with rebuilt ones whose digests differ. No revisit condition.
  - `auto` using the CUDA build only once it is installed, with a test load before committing to it: no precedent does either, and llama-server's `auto` detects and downloads. Revisit if GPU machines commonly want the CPU build, which `cpu` already gives them.
  - Runtime backend loading inside ggml, with one archive per platform: PromptForge's own builds do not use it, and whisper.cpp at `b4938` does not load backends itself, so it would need new FFI surface. Revisit if whisper.cpp starts loading its backends itself.
  - Compiling the Linux CUDA build on every push: the GitHub-hosted compile is slow, and master already keeps its GitHub-hosted Blackwell CUDA compile manual. Revisit if a self-hosted Linux runner becomes available.
  - A pull-request trigger for the workflow: it has none, and changes are checked by local dry runs before landing and by the push run after. No revisit condition.
  - An operator path override for the whisper library: llama-server has one (`llama_server_path` and `PROMPTFORGE_LLAMA_SERVER`), but it is a separate capability this work does not need, since both new builds are published. Revisit if an operator needs a build this repository does not publish.
  - A warning when the setting is set on a platform with one build: `llama_backend` has none, and the docs say where the setting is consulted. No revisit condition.
  - A config UI control: `llama_backend` has none either, and the Speech card already preserves the field. Revisit if the UI gains controls for build selection generally.
  - Matching the GPU's compute capability against the archive's native-code list: it couples selection to device code frozen at dispatch. Revisit if a second CUDA build with a different list is added. That condition was met, and the Windows row now matches, as the Decisions record.
  - Adding more native CUDA architectures: it departs from the Windows CUDA row, grows the archive, and cannot change after the dispatch. Revisit at the next whisper.cpp tag.
  - Refusing an explicit `cuda` below the floor: it blocks native-code GPUs that run on drivers 525 to 569. Revisit if crashes under an explicit `cuda` are reported.
  - A lower Windows CPU baseline: slower on every machine, different from the other x86 rows, and frozen at dispatch. No revisit condition.
  - Documentation alone for either crash: it leaves the process-ending path in place. No revisit condition.
  - A blanket `cpu` in the fixtures: CI's native job would fail until the Windows CPU row is pinned, and that runner's coverage would move to the CPU build. No revisit condition.
  - Running whisper out of process: a large redesign for two crash paths that selection can avoid. Revisit if another in-process abort path appears.
  - A separate cleanup plan run after this one closes: the user chose to fold the fixes in. No revisit condition.
  - Dispatching from the pull request's head before merge and pinning inside the pull request: replaced at the user's request. No revisit condition.
  - `auto` skipping an unpinned row, to close the window before the pin commit: master has no such rule, and one commit closes the window. Revisit if the window outlasts the first release dispatch after merge.
  - Never unloading the whisper runtime, by holding the library in `ManuallyDrop`: it changes the FFI's ownership on every build to work around one runtime, and its workaround comment would need an upstream issue URL. Revisit if a build without OpenMP still crashes on unload.
  - Keeping an extra reference to `vcomp140.dll`: it is process-global state in a serve path, which `AGENTS.md` forbids, and the Windows CPU build would still import the OpenMP runtime. No revisit condition.
  - Skipping the native free at process exit: it cannot stop an in-flight decode's CUDA calls, it breaks the speech engine's tested detach contract, and it needs process-global state to detect exit. No revisit condition.
  - Making the speech engine's `Drop` join its workers again: it undoes `6aa3409c`, which made `Drop` signal and detach so that no drop site blocks. No revisit condition.
  - Allowing reloads on the same thread, by setting `GGML_NO_BACKTRACE` or by patching ggml's terminate-handler check in the workflow: the variable is process-global state in a serve path, which `AGENTS.md` forbids, and the patch diverges from the pinned upstream source. Revisit if a product path ever reloads speech on the thread that loaded it.
  - Keeping the whisper library loaded after the retirement, so the published Windows CUDA build's OpenMP runtime never unloads: still rejected, for the reasons the never-unload entry above gives. The gate removes the `auto` exposure. Revisit if an explicit `cuda` on a Windows machine whose GPU CUDA cannot use must stop cleanly.
  - Rebuilding the Windows CUDA archive without OpenMP under a new name: it needs the self-hosted runner, a new pin, and a new pin in `.github/workflows/stt-miri.yml`, for an exposure the gate already closes under `auto`. Revisit at the next whisper.cpp tag.
  - Building the Linux CUDA archive against an older C++ runtime, in a container whose `libstdc++` predates GCC 12: CUDA speech would reach RHEL 9 and Amazon Linux 2023, but the dispatch-only job would change and need another GitHub-hosted run before merge. Revisit at the next whisper.cpp tag, or when those machines need CUDA speech.
  - A second Windows driver floor for GPUs without native code, at CUDA 13.3's driver: it adds a floor this machine cannot verify, and those GPUs would compile the PTX at every first use. Revisit if Turing, A100, or H100 Windows machines need CUDA speech under `auto`.
  - Asking the CUDA driver which devices it can use: it is new unsafe code outside the FFI crates. No revisit condition.
  - whisper's `encoder_begin_callback` as the abort: returning false stops the pass with partial text and success. No revisit condition.
  - The speech engine worker's stopping flag as the abort signal: it is set only after the drain wait. No revisit condition.
- Assumptions, risks, and notes:
  - The GitHub-hosted Linux CUDA compile is slow. The GitHub-hosted Windows CUDA compile took 95 minutes before that row moved to the self-hosted runner.
  - At the post-merge release dispatch, a failed build publishes nothing, because publishing waits for every build, and the dispatch is re-run after a fix.
  - A release dispatch rebuilds the five existing rows too, and the add-only publish discards them.
  - The post-merge dispatch rebuilds the Windows CUDA row under Step 9's check. The published Windows CUDA archive is 523 MB, which only a bundled cuBLAS explains, so the self-hosted runner's toolkit likely holds all three DLLs. If it lacks one, the dispatch fails at that check and publishes nothing.
  - A Windows CPU build without OpenMP may transcribe at a different speed, and Step 10 measures it.
  - ggml's load-time check, `GGML_ASSERT(prev != ggml_uncaught_exception)` in `ggml/src/ggml.cpp` at whisper.cpp `371b5a75`, aborts a reload of `ggml-base.dll` on a thread that loaded it before.
    - The cause: on Windows the C++ terminate handler is per thread, so the reload finds the handler the previous load left behind.
    - It affects every Windows build, the published CUDA archive included.
    - No product path reaches it. The gateway loads speech once per process, and Rust's built-in test harness gives each test its own thread.
  - A stop while a CUDA speech load outlasts the shared deadline still abandons the retirement, so that rare stop can still race the CUDA runtime's teardown. A load has no abort, and the decode abort does not cover it.
  - Before the decode abort, decodes could outlast the deadline too. Of four Linux CUDA stops with six 11-minute requests in flight, one printed `CUDA error: driver shutting down` and one exited 139. Windows exited 0 in three of three, because process exit ends the decode thread before the CUDA runtime unloads.
  - With the abort, a request whose decode is aborted fails, as admitted requests already do at shutdown. In the long-decode stops, every request still queued at the drain bound got HTTP 500.
  - The published Windows CUDA archive keeps OpenMP. Its decodes run on the GPU, and CI's native job, which loads and frees it test after test, passes.
  - `auto` depends on `nvidia-smi` being on the gateway's PATH. WSL2 keeps it in `/usr/lib/wsl/lib`, which systemd's default service PATH lacks, so a WSL2 service needs `cuda` set, or that directory on its PATH.
  - A card without native code compiles the PTX at its first decode. This was measured on 2026-10-02 with the Linux CUDA package in WSL on an RTX 3090, its sm_86 code relabeled so the GPU had to compile compute_80 PTX.
    - One native test took 22.8 s with an empty JIT cache, against 1.0 s with native code, and 1.1 s once the 44 MB cache was warm. With the cache disabled, every run took 22.5 s.
    - The shipped systemd unit's `DynamicUser` probably leaves no writable home for the cache. This was not verified.
  - On Windows with GPUs hidden (`CUDA_VISIBLE_DEVICES=-1`), the published CUDA build's CPU fallback crashed at a graceful stop after transcriptions in 10 of 10 runs. The gateway built at Step 11 exited 0 in the same five cases, and idle stops and stops with GPUs visible exited 0.
  - The Linux CUDA archive's `GLIBCXX_3.4.30` comes from `std::condition_variable::wait` in `libggml-cuda.so.0`. Both Linux jobs compile with GCC 11.4, and Ubuntu 22.04's GCC 12 `libstdc++` binds that symbol's newest version at link time.
  - The CUDA 12.8 floor is driver 570, and CUDA's minor-version compatibility from driver 525 cannot compile newer PTX, per NVIDIA's compatibility documentation as read on 2026-09-29. Neither crash was reproduced on real hardware.
  - The implementation reads the baseline extensions from the pinned ggml CMake rather than assuming a list.
  - Older debt stays recorded and unfixed:
    - the MSVC runtime that the Windows archives import without bundling it;
    - `speech.gpu` reporting how a build was compiled even when ggml finds no device;
    - the test-only eager gateway constructor that provisions speech before binding.
  - The `workshop-gateway` fixture race is pre-existing and tracked apart from this plan.
  - The post-merge release needs someone with write access to `cppalliance/promptforge` to dispatch it, and the self-hosted Windows CUDA runner online.
  - wg21-website's Talktron gateway guide, `docs/talktron-gateway.md`, depends on this work.
    - Today it installs the CUDA build by setting `cuda` and returning to `auto`. With detection, a native Linux machine with driver 570 or later leaves `auto`, and the WSL2 reference machine sets `cuda` or extends the unit's PATH.
    - Its checks read the `provisioned whisper library` line for the build. The path names it on stdout and in the journal but is redacted in `gateway.log`, where `/admin/status`'s `speech.gpu` is the check that holds.
    - Its revision table must name the post-merge pin commit, so the website PR that carries it lands after that commit.
  - Local branches `whisper-lib-linux-cuda` and `whisper-cuda-variant` already exist and are not part of this work. New branches use other names.

## Project survey

Surveyed at `a7e50ec5` on `whisper-cuda-backend` (clean tree). Architecture anchor: `vibe/archdoc.md` (components, invariants A1 to A9), read whole.

- Build command:
  - `cargo build --locked -p gateway`; plain `cargo build` builds the same `default-members` crate, `crates/gateway/app`. Default features are `local`, `web-search`, `config-ui`, `stt`; `config-ui` bundles `crates/gateway/config-ui/ui` with esbuild, so `npm ci --prefix crates/gateway/config-ui/ui` must have run (its `node_modules` is present on this machine).
  - Headless shape: `cargo build --locked -p gateway --no-default-features`. Desktop app: `cargo workshop` (alias of `cargo run -p build-workshop --`) after `npm ci --prefix crates/workshop` (its `node_modules` is absent on this machine).
  - Toolchain: stable per `rust-toolchain.toml` (cargo and rustc 1.98.0 here), edition 2024, resolver 3, no `rust-version` declared. Windows links with `rust-lld` and `+crt-static` (`.cargo/config.toml`).
  - On this machine every cargo command runs in WSL2 through `bash vibe/scratch/wsl-cargo.sh <cargo args>` (Ubuntu-24.04, cargo 1.98.1, target directory `~/promptforge-verify-target`, with `RUSTFLAGS` and `RUSTDOCFLAGS` passed through). `crates/gateway/config-ui/ui/node_modules` is a Linux install that the operator's WSL Talktron gateway build uses, so a Windows build of `gateway-config-ui` finds no `esbuild.cmd` and fails; the operator chose to keep it. `cargo xtask site --books-only` still runs on Windows, where mdBook 0.4.44 is installed.
- Focused test command pattern:
  - CI form: `cargo nextest run --locked -p <package> --all-features [<filter>]`. cargo-nextest is not installed on this machine (`cargo nextest` is "no such command"), so run `cargo test --locked -p <package> --all-features [--lib | --test it] [<module-path filter>]`.
  - This plan's areas: `cargo test --locked -p gateway-config --lib -- config::tests:: config::stt::tests::` (submodules `validation::`, `schema::`, `serialize::`, plus the inline tests in `config/stt.rs`, which `config::tests::` alone does not match), `cargo test --locked -p gateway-local --lib artifacts::assets::tests::`, `cargo test --locked -p gateway-local --lib artifacts::tests::`, `cargo test --locked -p gateway-stt --all-features --lib artifacts::tests::`.
  - One crate's doctests: `cargo test --locked --doc -p <package>`.
  - Native whisper tests are `#[ignore]`d; they run as `-- --ignored --test-threads=1` with `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO` set, as `.github/workflows/stt-miri.yml` does on the self-hosted Windows CUDA runner.
- Full-suite test command:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; the workshop trio runs apart as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - Without nextest: `cargo test --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` also runs the doctests, but loses `.config/nextest.toml`'s `heavy` group that throttles `gateway-stt` and `gateway-stt-backend-whisper`.
  - Without nextest, `workshop-gateway`'s fixture tests also race into ETXTBSY (`Text file busy`) under the threaded runner, a pre-existing copy-then-exec race (https://github.com/rust-lang/rust/issues/114554). So that crate runs apart with `--test-threads=1`.
  - Structural checks: `cargo test -p build-xtask`.
- Linter and formatter commands:
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - `cargo fmt --all --check` (`rustfmt.toml`: `style_edition = "2024"`). Headless gate: `cargo check -p gateway --no-default-features`.
  - On this machine the WSL toolchain has no clippy or rustfmt component. So `cargo fmt --all --check`, and clippy for crates that do not build the config UI, such as `gateway-config`, `gateway-local`, and `gateway-stt`, run with the Windows toolchain.
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
  - `crates/` root, the public layer: `promptforge` (Engine facade), `harness` (Harness facade), `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS package, not a crate), `workspace-hack` (cargo-hakari), and the `build-*` tools (`build-xtask`, `build-user-guide`, `build-ui`, `build-workshop`, `build-llama-cuda`).
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
  - Harness: `harness` -> `harness-sessions`, `harness-runner`, `harness-log`; sessions -> capabilities, models, runner, web; web -> webfetch, web-search. Every Harness crate reaches the Engine only through `promptforge` and names no gateway or workshop crate.
  - Gateway:
    - `gateway` (bin `promptforge-gateway`) -> config, logging, progress, protocol, routing, `gateway-api-types`, `gateway-api-discovery`, `shared-loopback`, and the optional `gateway-local`, `gateway-stt`, `gateway-web-search`, `gateway-config-ui`.
    - Speech: `gateway-stt` -> `gateway-local`, `gateway-config`, `gateway-progress`, `gateway-stt-engine`, `gateway-stt-backend-whisper`; backend-whisper -> `gateway-whisper-ffi`, engine, progress.
    - Below speech: `gateway-local` -> config, progress, protocol, routing, `shared-error-source`; routing -> config, protocol; protocol -> config, `gateway-api-types`; `gateway-config` -> `gateway-api-types` only.
    - Leaves: `gateway-whisper-ffi`, `gateway-stt-engine`, and `gateway-logging` have no workspace dependencies. No gateway crate names a promptforge, Harness, or workshop crate.
  - Workshop: `workshop` (desktop) -> `workshop-server-api` -> `workshop-server` -> features (`workspace`, `user-state`) -> services (`gateway`, `menu`, `status`) -> vocabulary (`protocol`, `registry`, `support`). The server also names `harness`, `promptforge`, `gateway-api-discovery`, `shared-loopback`.
  - Shared: `gateway-api-types`, `shared-error-source`, and `shared-loopback` have no workspace dependencies; `gateway-api-discovery` depends only on `shared-error-source`. The `build-*` crates depend on no workspace crate; `build-ui` is a build-dependency of `workshop-server` and `gateway-config-ui`.
  - Runtime: Workshop and the Harness reach the gateway over HTTP and WebSocket through the discovery file. Archdoc A1 (bind and report readiness before provisioning, which runs through the command queue) and A5 (local model set fixed for the process lifetime) govern speech provisioning.
  - Drift: the archdoc's CLI component has no crate here.
- Conventions summary:
  - Lints: `unsafe_code = "forbid"` workspace-wide; only `gateway`, `gateway-whisper-ffi`, `gateway-api-discovery`, and the desktop app carry their own lint tables, lowering it to `deny` and `pedantic` to warn (fatal under `-D warnings`). Each `unsafe` block has a `// SAFETY:` line. Clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, `missing_docs` warns. Members inherit `[lints] workspace = true` and `workspace-hack.workspace = true`.
  - Layout: source directories stay flat unless they hold at least three files; one or two become kebab siblings through `#[path]` (`server-support.rs`, `profile-name.rs`). The 500-line ceiling binds the 18 crates whose `lib.rs` carries `## Invariants` (every workshop-* and harness-internal crate) and `build-xtask` by its own invariant. Gateway crates carry no marker, so `artifacts.rs`, `runtime.rs`, and the test files legitimately run past 500 lines.
  - Config (`gateway-config`): public types are `#[non_exhaustive]` with private fields and `#[must_use]` accessors. Each section parses through a `pub(crate)` `RawX` with `#[serde(default, deny_unknown_fields)]`, validates in `TryFrom<RawX>`, serializes back through `From<&X> for RawX`, and routes `Deserialize` through the raw form. Enums take kebab or lowercase spellings, and a default value skips serialization through a predicate such as `LlamaBackend::is_auto`.
  - Artifacts (`gateway-local`): runtimes are digest-pinned rows (`ServerAsset`, `WhisperAsset`) in `crates/gateway/local/src/artifacts/assets.rs`, with lowercase-hex sha256 and `<os>-<arch>[-<variant>]` platform names. The literal `whisper-lib-b4938` appears only there and in `.github/workflows/stt-miri.yml`, which pins the Windows CUDA archive's hash. Windows child processes use `CREATE_NO_WINDOW`.
  - Errors: `thiserror` enums, `#[non_exhaustive]`, messages written for model consumption that name required versus actual. Comments state constraints only; a workaround cites its upstream issue URL.
  - Logging: `tracing` with structured fields. The gateway runs an unredacted stdout layer beside the `gateway.log` file layer (`init_logging_for_state` in `crates/gateway/app/src/main.rs`); the file layer never formats a classified field, `path` included, and replaces unlabeled local paths (`crates/gateway/logging/src/redact.rs`), so `provisioned llama-server`'s `path` reaches stdout but not `gateway.log`.
  - Workflows: top-level `permissions: contents: read`, with `contents: write` only on a publish job gated to `workflow_dispatch`. Third-party actions are pinned by commit SHA with a version comment, `actions/*` by major tag. Self-hosted jobs never run fork pull-request code. The GitHub-hosted CUDA compile is dispatch-only (`llama-cuda-blackwell.yml`: `windows-2022`, `timeout-minutes: 240`).
  - Guide: chapters are hand-edited, with no em-dash or double-dash, four-backtick fences, and one line per paragraph (`guide/CONTRIBUTING.md`); the `guide/promptforge-<set>-guide.md` exports come only from `cargo run -p build-user-guide`.
  - Line endings: `.gitattributes` forces LF, CRLF for `.ps1` and `.bat`.
  - Commits: imperative subject with no prefix, a prose paragraph on the behavior, then bullets naming the touched symbols and tests. Trailers are `Design: ...`, `Repairs: <what> @ <path>::<symbol> - <symptom>`, and `Plan: vibe/<plan>.md`. The first step's commit adds the exported plan and a `vibe/ACTIVE` file naming it; a final `Close plan: <words>` commit deletes `vibe/ACTIVE`.
  - Run state: the ledger is `vibe/scratch/vibe-ledger.md` and verification logs go under `vibe/scratch/logs/`, both ignored through `vibe/.gitignore` (`/scratch`); review findings go to the repository-root `vibe-review.md`, which the root `.gitignore` ignores. None of them is tracked, because the repository no longer keeps run ledgers in version control.
  - Remotes: `origin` is the fork `wpak-ai/promptforge`; `upstream` is `cppalliance/promptforge`.
  - Tooling on this machine: `gh`, `python`, WSL2 Ubuntu-24.04, `nvidia-smi` with two RTX 3090 GPUs, and CMake only inside Visual Studio 18 (`C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe`, not on PATH).
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
  3. Backend-aware whisper provisioning in `gateway-local` and `gateway-stt`, with its docs, the explicit fixture backend, the machine-capability gate, and the markers on the two unpinned rows, Steps 5 to 8. It comes after the setting because it uses the setting's type, and its new rows install only once the post-merge commit pins their archives.
  4. Release pipeline follow-ups in `.github/workflows/whisper-lib.yml`, Steps 9 and 10: the Windows CUDA package's runtime check, and the Windows CPU build without OpenMP. They return to the pipeline after the other components, because the user added them once Steps 1 to 8 were planned.
  5. Speech retirement before exit, Steps 11 and 12: the native whisper suite ends its speech engines with a joined shutdown, and `gateway`'s `Gateway::serve` retires speech at a graceful stop. It comes after Step 10, because both move the library's unload before the process exits, and the Windows CPU build survives that only without OpenMP.
  6. Machine-requirement gates for `auto` in `gateway-local`, Steps 13 and 14. They come first among the fixes because they close a crash this work introduced, and their tests need no GPU.
  7. Prompt stops, Steps 15 to 17, after component 6: the boot command's token cancels speech downloads in `gateway-local` and `gateway-stt`, and admission's shutdown aborts running decodes across `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-whisper-ffi`. They follow component 6 because they share none of its code or tests, and they finish the retirement Step 12 began.
- Pieces:
  - The pipeline's three pieces, Steps 1 to 3, are built one after another, add-only publish first. Each has its own check, and with the publish first no commit pairs the new builds with a publish that replaces archives.
  - The setting, Step 4, is one piece, covered by the config tests.
  - Selection, provisioning, and `prepare()` are built together as Step 5, because the new `whisper_asset` and `provision_whisper_library` signatures break the provisioning tests and `prepare()` until all of them change. The docs join that step because they describe its behavior.
  - The fixes after Step 5 are two pieces, built one after another because neither uses the other's code:
    - The explicit fixture backend, Step 6, goes first. It takes the native suites off the machine's GPUs, so Step 7 can run them on this Linux GPU machine as the real-machine check of its CPU detection.
    - The machine-capability gate with its docs, Step 7, is one piece. The driver floor and the baseline check change the same `whisper_asset` signature and the same selection tests, which building them apart would rewrite twice.
  - The markers, Step 8, follow alone. They leave the two rows for the post-merge pin commit, which makes the paths Steps 6 and 7 fix reachable.
  - The runtime check, Step 9, is one piece. Its edit already sits uncommitted in the working tree, so the step verifies and commits it.
  - The Windows CPU build without OpenMP, Step 10, is one piece: one configure flag and its checks. It follows Step 9 rather than joining it, because the two change different rows and neither uses the other.
  - Speech retirement is two pieces, built one after another, because neither uses the other's code and each has its own tests:
    - The native whisper suite's speech engine shutdown, Step 11, goes first. Its native runs cover it. On the Linux CUDA package they confirm at the speech engine level that joining the workers before exit removes the CUDA error, which is the mechanism Step 12 reaches through `SpeechService::shutdown`. On the pinned Windows CUDA build, which keeps OpenMP, they test that an in-process unload is safe before Step 12 relies on it.
    - The retirement in `Gateway::serve`, Step 12, is one piece: the change to `serve`, its doc comments, its unit tests, and the one realtime test that serves a single speech service from two servers, which the change would otherwise break.
  - The gates are two pieces, built one after another:
    - First come the row data and the pick, Step 13: the Windows floor and native list, which change only `assets.rs` and its selection tests, with their docs.
    - Then come the machine inputs and their readers, Step 14: hidden GPUs and the Linux C++ runtime. They add arguments the first piece's tests would otherwise rewrite twice.
  - The download cancellation, Step 15, is one piece, one token threaded through two crates. It shares nothing with the decode abort.
  - The decode abort is two pieces, built one after another, speech engine side first:
    - First, Step 16: every decode request carries the epoch's flag, which proves with scripted decoders that the retirement waits on the flag.
    - Then, Step 17: whisper's `abort_callback` reads the flag, with native tests. This piece uses the first piece's request accessor.
- Landing:
  - The work happens on branch `whisper-cuda-backend`, fast-forwarded to `master` at `a7e50ec5` before Step 1. `/export-vibe-plan` runs before Step 1.
  - `/export-vibe-plan` runs again before Step 6, before Step 8, before Step 10, and before Step 13. Each time it rewrites the plan's repository copy at its existing path, `vibe/2026-09-28-2-whisper-cuda-backend.md`, and the rewrite rides in that step's commit. The export before Step 10 carries Steps 10 to 12, and the export before Step 13 carries the fixes from the end-to-end testing.
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
  - The older debt listed under the Decision Record's notes, running whisper out of process, and further changes to `whisper-lib.yml`'s build steps or its CUDA architecture list beyond Steps 2, 3, 9, and 10.
  - The residuals the fixes leave, which the docs record:
    - an explicit `cuda` on a Windows machine whose GPU CUDA cannot use decodes on the CPU and ends the gateway at a graceful stop;
    - a stop while a speech load outlasts the deadline still abandons the retirement.
  - Issues the end-to-end testing found that predate this work, tracked apart:
    - the gateway log dropping a failed command's cause;
    - the Windows native fixtures' temporary caches;
    - the batch route's status for a full worker queue;
    - the guide's Linux system libraries;
    - SIGTERM and Ctrl-Break skipping the graceful stop.

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

- The `build` job's matrix gains `platform: windows-x86_64` on `windows-2022`, the GitHub-hosted image `.github/workflows/llama-cuda-blackwell.yml` builds on. It builds on push like the other rows.
- A new `Configure Windows CPU` step uses the `Configure Windows CUDA` flags without `-DGGML_CUDA=ON`.
- `Package Windows runtime` and `Smoke-load Windows runtime` run on both Windows rows, and only the CUDA row copies the CUDA runtime DLLs. `Package Unix runtime` and `Smoke-load Unix runtime` skip both Windows rows.
- Checks:
  - Before the commit, the row's configure, package, and smoke-load steps run under the ignored `vibe/scratch/` on a Windows machine. The zip holds `whisper.dll`, the `ggml*.dll` libraries, `whisper.h`, and `LICENSE`, and no CUDA DLL, and the smoke-load succeeds.
  - The first push run on `master` after Steps 1 to 3 land builds the row, and no other row changes.
- Dry run, 2026-09-28, on the operator's Windows machine with Visual Studio 18's CMake and MSVC 19.51 (the GitHub-hosted runner has Visual Studio 2022):
  - The row's configure, build, package, and smoke-load steps all ran, and the smoke-load printed CPU-only system information (AVX2, OpenMP).
  - The zip holds `whisper.dll`, `ggml.dll`, `ggml-base.dll`, `ggml-cpu.dll`, `parakeet.dll`, `whisper.h`, and `LICENSE`, with no CUDA DLL although the machine has `CUDA_PATH` set, and its `.sha256` line matches.
  - A replay of the CUDA row's package step against a stub toolkit bundles exactly what it did before the change.
  - Tag `b4938` (the same commit as `v1.9.3`) also builds a `parakeet.dll` that `whisper.dll` does not import. The shared Windows `*.dll` packaging bundles it into both Windows archives, as the published CUDA archive already holds it, and the Unix filter leaves it out.

### Step 3: Build a Linux x86-64 CUDA runtime on release dispatch only [completed]

- A new `build-linux-cuda` job in `.github/workflows/whisper-lib.yml` runs only when `github.event_name == 'workflow_dispatch'`, on `ubuntu-22.04`, with `timeout-minutes: 240`, as the GitHub-hosted Blackwell build has.
- Its steps:
  - Check out whisper.cpp and clean staging as the `build` job does.
  - Install NVIDIA's CUDA 12.8 apt components: the compiler, cudart, cuBLAS, and the driver stubs.
  - Configure with the `Configure Linux` flags plus `-DGGML_CUDA=ON` and no `CMAKE_CUDA_ARCHITECTURES`, then build and install to `stage`.
  - Package `whisper-${WHISPER_TAG}-linux-x86_64-cuda.zip` and its `.sha256`: each ggml library once under its soname, `libwhisper.so` under the name the gateway opens, `libcudart.so.12`, `libcublas.so.12`, `libcublasLt.so.12`, `whisper.h`, and `LICENSE`.
  - Smoke-load `libwhisper.so` with the toolkit's stub `libcuda.so.1`. `whisper_print_system_info()` must report CUDA, and `ldd` must resolve nothing outside the package except glibc's libraries, the C++ runtime (`libstdc++` and `libgcc_s`, which the CPU archive already needs), the OpenMP runtime, and the driver.
  - Upload the archive as `whisper-linux-x86_64-cuda-${WHISPER_TAG}`, which the publish job's download pattern matches.
- The `publish` job's `needs` gains `build-linux-cuda`.
- Checks:
  - Before the commit, the job's shell steps run in a scratch directory outside the tracked tree on a Linux x86-64 machine, publishing nothing; WSL2 is enough. The build succeeds, the stub smoke-load reports CUDA, and `ldd` resolves every package library inside the package. A container with no driver refuses the load on `libcuda.so.1`. The step's record in the plan's repository copy notes the machine, the build time, and the architecture list ggml chose.
  - Optionally, a fork dispatch of the branch exercises both GitHub-hosted jobs and is cancelled while the self-hosted Windows CUDA row waits, so nothing publishes. On 2026-09-30 both new builds compiled on GitHub-hosted runners this way, the Linux CUDA job in 82 minutes.
  - The first push run on `master` skips the job.
- Dry run, 2026-09-28, in WSL2 Ubuntu 24.04 on an i9-14900KF (32 threads, 31 GB), with gcc 13.3.0, CMake 3.28.3, and nvcc 12.8.93; tag `b4938` resolves to whisper.cpp `371b5a75` (ggml 0.20.2):
  - Every `run:` block of the job exits 0. `cmake --build` takes about 470 s at `-j32` (12,244 CPU-seconds); a 4-vCPU GitHub-hosted runner should take 50 minutes or more, which was not measured. The build passes `--parallel "$(nproc)"`, because a bare `--parallel` starts every nvcc compile at once.
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
  - In `crates/gateway/local/src/artifacts/tests.rs`, `provision_whisper_library_reuses_a_verified_install` and `whisper_installs_never_fall_back_to_an_older_abi` pass an explicit backend, so no test probes the machine's GPUs.

### Step 6: Give the native speech fixtures an explicit whisper backend [completed]

- `crates/gateway/stt/api/src/test_fixtures/native.rs` gains a public `fixture_whisper_backend()` beside its `require_fixture` re-export. It returns the `[stt] whisper_backend` spelling from the test-only `PROMPTFORGE_WHISPER_BACKEND`, and `cpu` when that is unset. It returns the spelling rather than a `WhisperBackend`, so an unknown value fails the fixture's config parse with the error that names the accepted values.
- The three native fixture configs write that spelling into `[stt]`:
  - `fixture_service_with_models_on_dedicated_thread` in `crates/gateway/stt/api/tests/common/mod.rs`;
  - `verbose_round_trip_accepts_literal_timestamp_granularities_field` in `crates/gateway/stt/api/src/batch-native-tests.rs`, whose config gains an `[stt]` section;
  - `native_speech_service` in `crates/gateway/app/tests/it/realtime_stt.rs`.
- In `.github/workflows/stt-miri.yml`, the `native-whisper` job's `Provision pinned native fixtures` step adds `PROMPTFORGE_WHISPER_BACKEND=cuda` to `$env:GITHUB_ENV` beside the other fixture variables, so the self-hosted Windows CUDA runner keeps testing the CUDA build.
- Checks, with the `PROMPTFORGE_WHISPER_*` fixture variables set as `stt-miri.yml` sets them:
  - Before the change, on this machine in WSL with `nvidia-smi` on PATH, the ignored native suites fail, because `auto` selects the unpublished, all-zero `linux-x86_64-cuda` row.
  - After it, without `PROMPTFORGE_WHISPER_BACKEND`, they pass on the pinned `linux-x86_64` CPU build: `cargo test --locked -p gateway-stt --lib -- --ignored --test-threads=1`, the same with `--test it`, and `cargo test --locked -p gateway --test it realtime_stt::realtime_stt_native_incremental -- --ignored --test-threads=1`.
  - Until the post-merge commit pins the Windows CPU row, a Windows run of those suites sets `PROMPTFORGE_WHISPER_BACKEND=cuda`, as CI does.
  - CI's `native-whisper` job skips fork pull requests, so it first runs after merge. It passes on the CUDA build.

### Step 7: Gate whisper selection on the machine's driver and CPU [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - A new `NvidiaProbe` holds the probe's answer: each GPU's compute capability, and the driver version's major number, `None` when it cannot be read.
  - `WhisperAsset` gains `min_driver_major: Option<u64>`: `Some(570)` on `linux-x86_64-cuda`, the CUDA 12.8 floor, and `None` on every other row, `windows-x86_64-cuda` included.
  - `auto_whisper_backend` picks `Cuda` only when the probe reports a GPU and the platform's CUDA row has no floor or a floor at or below the driver's major number. An unreadable version counts as below any floor.
  - `X86_BASELINE` lists the extensions whisper.cpp `371b5a75` enables under `GGML_NATIVE=OFF`: `sse4.2`, `avx`, `avx2`, `bmi2`, `fma`, and `f16c`. They come from the `INS_ENB` options in `ggml/CMakeLists.txt` and the x86 flags in `ggml/src/ggml-cpu/CMakeLists.txt`, where MSVC's `/arch:AVX2` implies FMA and F16C. The list is tied to `WHISPER_RELEASE`.
  - `whisper_asset(os, arch, backend, gpus, x86_extensions)` takes the probe's answer as `Option<&NvidiaProbe>` and the machine's detected baseline extensions. On `x86_64`, under every setting, a missing baseline extension returns `LocalError::UnsupportedCpu` in place of the row; other architectures skip the check.
  - `whisper_asset_with_probe` passes both through, and still probes only for `auto` on Windows x86-64 and Linux x86-64.
- In `crates/gateway/local/src/artifacts.rs`:
  - `nvidia_compute_caps()` becomes `nvidia_probe()`: one `nvidia-smi --query-gpu=compute_cap,driver_version --format=csv,noheader` run, still without a console window on Windows, whose output a new pure `parse_nvidia_probe(stdout)` reads.
  - `provision_llama_server_with_cancellation` hands `server_asset` only the probe's compute capabilities, so `server_asset`, `auto_backend`, and their tests stay as they are.
  - A new `machine_x86_extensions()` returns the `X86_BASELINE` members `std::arch::is_x86_feature_detected!` reports, under `#[cfg(target_arch = "x86_64")]`, and none elsewhere.
  - `ArtifactStore::provision_whisper_library` passes `nvidia_probe` and `machine_x86_extensions()` to selection, and its `# Errors` section names the new variant.
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
  - In `crates/gateway/local/src/artifacts/tests.rs`, `parse_nvidia_probe` reads `compute_cap, driver_version` lines, this machine's recorded output among them, and answers `None` for no GPU. `provision_whisper_library_reuses_a_verified_install` and `whisper_installs_never_fall_back_to_an_older_abi` pass the full baseline to `whisper_asset`.
  - llama-server's selection tests pass unchanged.
- Checks:
  - Before the commit, the query runs on this machine under Windows and under WSL, and its lines become parser cases.
  - `cargo test --locked -p gateway-local --lib artifacts::` and `cargo test --locked -p gateway-stt --all-features --lib artifacts::tests::` pass.
  - With Step 6's fixtures, the ignored native suites still pass on this machine's CPU build, which runs `machine_x86_extensions()` on a real CPU. `provision_whisper_library_reuses_a_verified_install` now runs that check too, so on an x86-64 machine below the baseline it fails with the named error.

### Step 8: Leave the two new pins for after merge [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`, the `windows-x86_64` and `linux-x86_64-cuda` rows keep their all-zero digests and gain the placeholder comment that `WINDOWS_X86_64_CUDA_BLACKWELL` carried from `66f8f95f` until `56feb2dd` pinned it. The comment says the pin is filled in once the `whisper-lib-b4938` release holds the archive, and that until then the row is fail-closed: the pin can never match, so the download is refused rather than trusted.
- Nothing else in the code changes. The plan's re-export, which moves the pins after merge, rides in this step's commit.
- Checks: the seven-row coverage test and the selection tests pass unchanged, because the comments change no behavior. As the last step of its component, it runs the full suite.

### Step 9: Require the CUDA runtime DLLs in the Windows CUDA package [completed]

- In `.github/workflows/whisper-lib.yml`, the `windows-x86_64-cuda` branch of `Package Windows runtime` checks, after it copies the CUDA runtime DLLs, that the bundle holds a `cudart64_*.dll`, a `cublas64_*.dll`, and a `cublasLt64_*.dll`. A missing one throws `<pattern> missing from the runtime bundle`, the message the step's `ggml*.dll` check already uses.
- The copy still searches the toolkit with `-ErrorAction SilentlyContinue`, so the check is what turns a missing DLL into a failed row instead of a bundle without it.
- The CPU row never enters that branch, so its package step is unchanged, and the existing `whisper.dll` and `ggml*.dll` checks stay.
- The edit already sits uncommitted in the working tree. This step verifies it and commits it.
- Checks, in scratch on this Windows machine, replaying the step with Step 2's dry-run scripts:
  - against a stub toolkit holding all three DLLs, the CUDA row's package step bundles them and passes;
  - with each DLL removed from the stub in turn, the step fails naming that pattern;
  - the CPU row's package step bundles exactly what it did before;
  - the CUDA branch's copy and check also pass against this machine's own CUDA toolkit at `CUDA_PATH`.
- It changes no published archive, because the add-only publish never replaces one.

### Step 10: Build the Windows CPU runtime without OpenMP [completed]

- In `.github/workflows/whisper-lib.yml`, `Configure Windows CPU` gains `-DGGML_OPENMP=OFF`, the option whisper.cpp `371b5a75` declares in `ggml/CMakeLists.txt`. Master's Windows llama-server build sets the same flag in `configure_options`, in `crates/build-llama-cuda/src/cmake.rs`. Nothing else in the workflow changes, and the Windows CUDA row keeps OpenMP.
- With OpenMP on, `ggml-cpu.dll` imports the MSVC OpenMP runtime, `VCOMP140.DLL`. Freeing `whisper.dll` after a CPU transcription unloads that runtime while its thread team is still alive, and the next time one of those threads runs, the process ends with an access violation. Without OpenMP, ggml runs its own thread pool.
- The commit also carries the plan's re-export, as the Landing sets out.
- Checks, in scratch on this Windows machine, with Step 2's dry-run scripts (`vibe/scratch/whisper-cpu-dryrun/`) building the row with Visual Studio 18's CMake and MSVC:
  - the package holds what Step 2's record lists, and `dumpbin /dependents` over every DLL in it lists no `VCOMP140.DLL`;
  - the smoke-load's `whisper_print_system_info()` output has no `OPENMP` entry, where Step 2's build reported `OPENMP = 1`;
  - a probe that, on a fresh thread for each cycle, loads the library, transcribes `jfk.wav` on the CPU, and frees it, several times over, exits cleanly, where the OpenMP build ends with `0xC0000005` at the first free. One thread per load matches how the gateway and Rust's built-in test harness load the library;
  - the ignored native suites of `gateway-whisper-ffi` and `gateway-stt-backend-whisper` pass with `PROMPTFORGE_WHISPER_LIBRARY` at the new `whisper.dll`;
  - a `small.en` transcription of `jfk.wav`, timed on the OpenMP and OpenMP-off builds, is recorded under this step.
- If the probe still ends the process without OpenMP, the step stops and reports, and Steps 11 and 12 wait.
- Dry run, 2026-10-01, on this machine with Visual Studio 18's CMake and MSVC 19.51, against Step 2's OpenMP build as the control:
  - With OpenMP, `ggml-base.dll` and `ggml-cpu.dll` import `VCOMP140.DLL`, the smoke-load reports `OPENMP = 1`, the probe ends with `0xC0000005` at the first free, and the `native_whisper` suite crashes with `STATUS_ACCESS_VIOLATION`.
  - With `-DGGML_OPENMP=OFF`, no DLL imports `VCOMP140.DLL` and the system info has no `OPENMP` entry.
    - The probe, one thread per cycle, exits cleanly: 6 of 6 cycles in its first run, and 5 of 5 in the dry-run scripts' `noomp` phase.
    - The native suites pass: 1 test in `gateway-whisper-ffi`, then 5 and 2 in `gateway-stt-backend-whisper`.
    - Step 9's `runtime` checks pass 6 of 6.
  - A `small.en` transcription of `jfk.wav`, on 4 threads with the median of 5 runs, took 1.997 s and 1.928 s over two rounds with OpenMP, and 1.940 s and 1.944 s without. There is no material difference.
  - A reload on the thread that loaded the library before aborts with `0xC0000409` in ggml's own load-time check, at the second cycle in two runs and the fourth in a third. The Decision Record notes this upstream limitation.

### Step 11: Shut down the native whisper suite's speech engines before each test ends [completed]

- In `crates/gateway/stt/backend-whisper/tests/native_whisper.rs`, each of the five tests that builds an `SttEngine` now ends it with `SttEngine::shutdown()` and expects `Ok`. Today each one drops its speech engine, either explicitly or at the end of its scope:
  - `packaged_runtime_preserves_native_transcription_contract` shuts down `glossary_prompted` and `unprompted` before it removes its copied model;
  - `independent_final_jobs_do_not_require_a_reset`, `one_final_job_cannot_change_another_jobs_history`, and `final_decode_is_absent_without_a_final_model` each shut down their one speech engine after their assertions;
  - `configured_model_branches_write_their_load_text_then_release_the_activity` shuts down its speech engine after the activity check.
- `shutdown()` joins both workers, so each test releases its decoders and its library reference before it returns. The test process then exits with no worker still freeing whisper state.
  - On the Windows CPU build, each test unloads the library inside the process after CPU transcriptions, which is the path Step 10 makes safe.
  - The `gateway-stt` native suites already end their services with `SpeechService::shutdown`, and this suite now does the same at the speech engine level.
- The speech engine, its signal-and-detach `Drop`, the FFI, and the suite's assertions do not change.
- The pinned Windows CUDA archive, `whisper-b4938-windows-x86_64-cuda.zip` (523,565,776 bytes) from the `whisper-lib-b4938` release, is downloaded once into the ignored `vibe/scratch/` for this step's and Step 12's checks; approving this plan authorizes that download. No local copy exists on this machine.
- Checks run `cargo test --locked -p gateway-stt-backend-whisper --test native_whisper -- --ignored --test-threads=1`, with `PROMPTFORGE_WHISPER_MODEL` and `PROMPTFORGE_WHISPER_AUDIO` at the fixtures in `vibe/scratch/stt-native/`:
  - In WSL, with `PROMPTFORGE_WHISPER_LIBRARY` at `libwhisper.so` in the extracted Linux CUDA package from Step 3's dry run (`~/whisper-cuda-dryrun/work/dist/`). One run before the change is recorded under this step, whether or not it prints a `CUDA error` at exit. After the change, the suite passes and exits 0 with no `CUDA error` in its output.
  - In WSL, on the pinned `linux-x86_64` CPU build, the suite passes.
  - On this Windows machine, with the Windows toolchain and Step 10's OpenMP-off `whisper.dll`, the suite passes. A run on Step 2's OpenMP build is recorded beside it as the control.
  - On this Windows machine, on the `whisper.dll` of the pinned `windows-x86_64-cuda` build, which keeps OpenMP, the suite passes. CI's `native-whisper` job runs this suite on that build, but only after merge.
- The step stops and reports, and Step 12 waits, if either of these happens:
  - the suite still prints a `CUDA error` at exit on the Linux CUDA package;
  - the suite ends the process on the pinned Windows CUDA build.
- Runs, 2026-10-01:
  - Linux CUDA package, before the change: four tests passed. The process then aborted with SIGABRT during the last test, after four `CUDA error` lines (`ggml-cuda.cu:106`) from `whisper_free_state` on the detached workers, and cargo exited 101.
  - Linux CUDA package, after the change: 5 passed, exit 0, and no `CUDA error`, in 4 of 4 runs.
  - Pinned Linux CPU build: 5 passed.
  - Windows CPU build without OpenMP: 5 passed, in 3 of 3 runs. Step 2's OpenMP control crashed with `STATUS_ACCESS_VIOLATION` in `independent_final_jobs_do_not_require_a_reset`, the first test that unloads the library after CPU transcriptions.
  - Pinned Windows CUDA build: 5 passed, in 3 of 3 runs.

### Step 12: Retire speech before the gateway exits [completed]

- In `crates/gateway/app/src/runner.rs`, `Gateway::serve` clones `state.speech`, the process's `SpeechService`, under the `stt` feature, before `build_router` takes the state.
- After the command worker's bounded join, `serve` runs `SpeechService::shutdown` on `tokio::task::spawn_blocking`:
  - It uses a deadline shared with that join. The deadline is `WORKER_JOIN_TIMEOUT` past one `tokio::time::Instant` read before the join, and `tokio::time::timeout_at` takes it for both waits.
  - On expiry, it logs a warning that names the bound and abandons the call, as the join does for a stuck command. An example message: `speech did not retire within {WORKER_JOIN_TIMEOUT:?}; abandoning it`.
- `SpeechService::shutdown` already closes admission, which ends Realtime sessions. It waits for the runtime to drain and then joins the speech engine's workers, so whisper frees its context while the CUDA runtime is still alive. The speech engine, its signal-and-detach `Drop`, and the FFI do not change.
- Doc comments:
  - the shutdown paragraph of `Gateway::serve` adds the retirement after the worker join, within the same deadline;
  - `WORKER_JOIN_TIMEOUT` and `RUNTIME_SHUTDOWN_TIMEOUT` say that the join deadline now also bounds the speech retirement;
  - `Gateway::with_speech_service` says that a graceful stop of `serve` retires the service it was given, so one service serves one `serve` call.
- In `crates/gateway/app/tests/it/realtime_stt/authentication.rs`, `gateway_auth_origin_query_and_final_speech_surfaces_precede_upgrade` gives its `trusted` server a scripted speech service of its own. Today it reuses the `strict` server's service. After this change the `strict` server's stop retires that service, so the `trusted` upgrade would get 503. No other test serves one speech service twice.
- Tests sit in `drain_tests`, beside `serve_abandons_a_worker_that_ignores_cancellation_after_the_join_bound`, each under `#[cfg(feature = "stt")]`. They use `Gateway::with_speech_service` and the `gateway-stt` test fixtures' `scripted_service` over an interim and a final `ScriptedDecoder`, and they need no GPU:
  - `serve_retires_speech_before_it_returns`: after a graceful stop, when `serve` returns `Ok`, both scripted decoders report their workers dropped and `SpeechService::status` reports not ready. Today's code fails this test.
  - `serve_abandons_a_speech_retirement_that_outlasts_the_join_bound`: a worker job is held through `generation_ownership(..).own_worker_job()` across the stop. `serve` returns `Ok` within one to two join bounds, and dropping the job afterward lets `wait_until_worker_dropped` succeed. Today's code fails this test.
- Checks, in scratch:
  - In WSL, a scratch clone at Step 11's commit pins its `linux-x86_64-cuda` row to the Linux CUDA package from Step 3's dry run (`~/whisper-cuda-dryrun/work/dist/`), served from loopback, and runs a gateway with `whisper_backend = "cuda"`.
    - Without this step's change, a stop with speech loaded prints the `CUDA error` that local testing found.
    - With the change, ten stops print no `CUDA error` and exit 0. They mix `POST /shutdown` and SIGINT, and some come after a transcription.
    - On `cpu`, which is the pinned Linux CPU build, the same clone also exits 0 when stopped after a transcription.
  - On Windows, with the Windows toolchain, a scratch clone pins its `windows-x86_64` row to Step 10's build, served from loopback. It builds the gateway without the `config-ui` feature, so it needs no Windows `node_modules`.
    - On `cpu`, the gateway exits 0 when stopped after a transcription.
    - On `cpu`, the `gateway-stt` native suites (`--lib` and `--test it`, ignored, one thread) pass on that build.
    - On `cuda`, the pinned Windows CUDA build, served from loopback with its real pin from Step 11's download, keeps OpenMP, and the gateway also exits 0 when stopped after a transcription.
  - The scratch work binds loopback ports clear of 8000, 8002, 8008, 8009, and 8011. It leaves `~/pf-target` untouched, only reads `~/.promptforge`, and pulls or publishes nothing beyond Step 11's one download.
- As the final step, it runs the full suite.
- Runs, 2026-10-01:
  - Linux CUDA package, without this step's change: both stops printed `CUDA error: driver shutting down` from `cudaFree`. One exited 0, the other 134, from an abort.
  - Linux CUDA package, with the change: 10 of 10 stops exited 0 with no `CUDA error`. Five were `POST /shutdown` and five SIGINT, and six came after transcriptions on both models. Both stops on the pinned Linux CPU build, each after a transcription, exited 0.
  - Windows CPU build without OpenMP: the gateway exited 0 after a transcription. The `gateway-stt` native suites passed: 1 test from `--lib` and 2 from `--test it`.
  - Pinned Windows CUDA build: both runs reported `gpu: true` and exited 0 after transcriptions.

### Step 13: Gate Windows `auto` on the CUDA build's driver floor and native GPU code [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - The `windows-x86_64-cuda` row's `min_driver_major` becomes `Some(580)`, with a comment naming it the CUDA 13 floor on Windows, as the Linux row's comment names CUDA 12.8's.
  - `WhisperAsset` gains `native_compute_caps: Option<&'a [(u64, u64)]>`, the GPU compute capabilities the build carries native code for, which only the `auto` pick consults; `None` checks nothing.
  - The field is `Some(&[(8, 6), (8, 9), (12, 0), (12, 1)])` on `windows-x86_64-cuda` and `None` on every other row, `linux-x86_64-cuda` included.
  - Its comment names the source, the pinned archive's `ggml-cuda.dll` fatbinary as read on 2026-10-02, whose PTX for compute_75, compute_80, and compute_90 is ISA 9.3 from CUDA 13.3. Like `X86_BASELINE`, the list is tied to `WHISPER_RELEASE`, and a release bump re-reads it from the new archive.
  - In each row literal the field sits beside `min_driver_major`, ahead of `platform`, because the scratch scripts' `patch-assets.py` matches `platform` directly followed by `archive`.
  - `auto_whisper_backend` takes `Cuda` only when the probe reports a GPU, the driver meets the CUDA row's floor, and, when the row lists native capabilities, every probed GPU's capability is in the list.
  - A comment there says why every GPU counts: whisper uses CUDA's device 0, whose fastest-first order need not match `nvidia-smi`'s.
  - The doc comments of `WhisperAsset`, `auto_whisper_backend`, and `whisper_asset` name the native-code requirement. `whisper_asset`'s signature does not change.
- Docs, including the first residual in the Deferred list:
  - `guide/src/gateway/05-speech.md`, under "The runtime": the Windows CUDA build needs driver 580 or later and carries native code only for compute capability 8.6, 8.9, 12.0, and 12.1, so `auto` takes the CPU build below that driver, at an unreadable version, or when any GPU has another capability.
  - The same paragraph gives an explicit `cuda`'s outcomes on Windows, with `auto` or `cpu` as the recovery: a GPU without native code under a driver older than CUDA 13.3's ends the gateway at the first transcription, and where CUDA finds no usable device the build decodes on the CPU and a graceful stop ends the gateway.
  - It also says that a GPU without native code, such as a Linux GPU at 7.5, 8.0, or 9.0, compiles the build's PTX at its first decode, which took about 20 seconds with an empty driver JIT cache.
  - `guide/src/gateway/04-local-models.md`, under "The cache directory": a CUDA build takes about 1.2 GB of cache on Windows and 1.7 GB on Linux, counting the downloaded archive the cache keeps.
  - The `whisper_backend` row of the `[stt]` table in `crates/gateway/app/README.md` and the commented `whisper_backend` lines in `gateway.local.example.toml` state the Windows floor, the native list, and the explicit `cuda` outcomes.
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
- The commit also carries the plan's re-export, as the Landing sets out.
- Tests, in `assets.rs`:
  - `auto_takes_the_windows_cuda_whisper_build_at_any_driver_version` becomes `auto_takes_the_windows_cuda_whisper_build_from_driver_580`, shaped like the Linux test: with a GPU at 8.6, an unreadable version, 0, or 579 selects `windows-x86_64`, and 580 or 591 selects `windows-x86_64-cuda`.
  - `auto_takes_the_windows_cpu_whisper_build_for_a_gpu_without_native_code`: at driver 591, a GPU at 7.5, 8.0, 9.0, or 6.1, alone or beside one at 8.6, selects `windows-x86_64`, while each native capability alone, and 8.6 beside 12.0, selects `windows-x86_64-cuda`.
  - `auto_takes_the_linux_cuda_whisper_build_for_any_gpu_above_its_floor`: at driver 570, a GPU at 7.5 still selects `linux-x86_64-cuda`, because that row lists no native capabilities.
  - `an_explicit_whisper_backend_ignores_the_probe` gains a probe below the Windows floor and one with a GPU at 7.5, so explicit `cpu` and `cuda` ignore both checks.
  - `whisper_assets_cover_the_seven_release_builds` asserts each row's `min_driver_major` and `native_compute_caps`.
- Checks:
  - `cargo test --locked -p gateway-local --lib artifacts::` passes, with llama-server's selection tests unchanged among them.
  - The end-to-end checks of Steps 13 to 17 run the scratch scripts in `vibe/scratch/run-36900875610/scripts/`, as the Testing Plan sets out, on their WSL clone `~/promptforge-e2e` and their Windows clone `vibe/scratch/pf-e2e`. The coder runs them before the step's commit exists, so each clone moves to the previous step's commit and takes the step's working-tree diff on top.
  - The scratch scripts `wsl-env.sh` and `win-env.sh` accept only `4b2ab768` today, so each step first names its own commit there. `wsl-prepare.sh` and `win-prepare.sh` then point the clones' two new rows at run 36900875610's archives, served from loopback, and build the gateways.
  - Windows, `win_gateway.py boots`: on this machine, whose two RTX 3090s report 8.6 on driver 591, `auto` still takes the CUDA build, and `cpu` takes the CPU build.

### Step 14: Gate `auto` on hidden GPUs and the Linux C++ runtime [completed]

- In `crates/gateway/local/src/artifacts/assets.rs`:
  - `WhisperAsset` gains `min_glibcxx: Option<&'a str>`, beside `native_compute_caps`: the `libstdc++` symbol version the build needs, which only the `auto` pick consults.
  - It is `Some("GLIBCXX_3.4.30")` on `linux-x86_64-cuda`, the version `libggml-cuda.so.0` needs for `std::condition_variable::wait`, and `None` elsewhere. Like the native list, it is tied to `WHISPER_RELEASE`.
  - A new `CudaMachine` holds what `auto` reads from the machine beside the probe: `visible_devices: Option<String>`, the `CUDA_VISIBLE_DEVICES` value, and `libstdcxx: Option<Vec<u8>>`, the machine's `libstdc++.so.6`, `None` when none was read.
  - `cuda_visible_devices_hides_every_gpu(value: Option<&str>, gpu_count: usize) -> bool` applies CUDA's rule that only the devices before the first invalid entry are visible: a set value hides every GPU unless its first entry is an index below `gpu_count` or a `GPU-` or `MIG-` identifier.
  - `libstdcxx_defines(library: &[u8], version: &str) -> bool` finds `version` followed by a NUL in the library's bytes, so a longer version that shares its prefix does not count.
  - `auto_whisper_backend` also requires that `CUDA_VISIBLE_DEVICES` leaves a probed GPU visible and, when the CUDA row names a `min_glibcxx`, that the machine's runtime defines it. No runtime read counts as lacking it.
  - `whisper_asset(os, arch, backend, gpus, cuda_machine: Option<&CudaMachine>, x86_extensions)` hands the machine readings to the pick.
  - `whisper_asset_with_probe` gains `cuda_machine: impl FnOnce() -> CudaMachine`, which runs only for `auto` where both builds exist, and only after the probe reports a GPU.
- In `crates/gateway/local/src/artifacts.rs`:
  - A new `machine_cuda()` reads `CUDA_VISIBLE_DEVICES` lossily, so a value that is not UTF-8 reads as an invalid entry, and reads the C++ runtime through `machine_libstdcxx()`.
  - A new `machine_libstdcxx()`, under `#[cfg(target_os = "linux")]` and `None` elsewhere, reads the first file that exists among `LIBSTDCXX_PATHS`: `/usr/lib/x86_64-linux-gnu/libstdc++.so.6`, `/usr/lib64/libstdc++.so.6`, `/lib/x86_64-linux-gnu/libstdc++.so.6`, `/lib64/libstdc++.so.6`, and `/usr/lib/libstdc++.so.6`. No file found, or a failed read, is `None`.
  - `ArtifactStore::provision_whisper_library` passes `machine_cuda` beside `nvidia_probe`, and its doc names both machine reads. llama-server's selection reads neither.
- Docs, including the hidden-GPU case of the Deferred list's first residual:
  - `guide/src/gateway/05-speech.md`, under "The runtime": on Linux, `auto` takes the CUDA build only where the machine's `libstdc++.so.6` comes from GCC 12 or later and defines `GLIBCXX_3.4.30`, so RHEL 9, its rebuilds, and Amazon Linux 2023 keep the CPU build, and an explicit `cuda` there fails to load.
  - The same paragraph says that on both platforms a `CUDA_VISIBLE_DEVICES` that hides every GPU counts as no GPU, and that an explicit `cuda` on Windows with every GPU hidden decodes on the CPU and ends the gateway at a graceful stop.
  - The `whisper_backend` row in `crates/gateway/app/README.md` and the commented lines in `gateway.local.example.toml` state both requirements.
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
- Tests, in `assets.rs`, where the selection tests move to the new signature with a C++ runtime that defines `GLIBCXX_3.4.30`:
  - `cuda_visible_devices_hides_every_gpu_by_cudas_rule`: with two GPUs, unset, `0`, `1,0`, `0,-1`, `GPU-...`, and `MIG-...` leave a GPU visible, and empty, `-1`, `2`, and `none` hide them.
  - `libstdcxx_defines_matches_only_the_whole_version`: it finds a NUL-terminated `GLIBCXX_3.4.30`, and rejects bytes that hold only `GLIBCXX_3.4.29` or only `GLIBCXX_3.4.300`.
  - `auto_takes_the_linux_cuda_whisper_build_only_with_glibcxx_3_4_30`: with a GPU on driver 591, a runtime that defines the version selects `linux-x86_64-cuda`, and one that lacks it, or no runtime, selects `linux-x86_64`. Windows ignores the runtime.
  - `gpus_hidden_from_cuda_select_the_cpu_whisper_build`: on both platforms, a value that hides every GPU selects the CPU row, and one that leaves a GPU visible keeps the CUDA row.
  - `an_explicit_whisper_backend_ignores_the_probe` gains hidden GPUs and a missing runtime, and `whisper_assets_cover_the_seven_release_builds` asserts each row's `min_glibcxx`.
  - `auto_probes_where_both_whisper_builds_exist_and_follows_the_answer` and `the_whisper_probe_runs_only_for_auto_where_both_builds_exist` also count the machine read: once under `auto` after the probe reports a GPU, and never otherwise.
- In `crates/gateway/local/src/artifacts/tests.rs`, `provision_whisper_library_reuses_a_verified_install` and `whisper_installs_never_fall_back_to_an_older_abi` move to the new signature.
- Checks:
  - `cargo test --locked -p gateway-local --lib artifacts::` passes.
  - Windows, `win_gateway.py hidden`: under `auto` with `CUDA_VISIBLE_DEVICES=-1` the gateway takes the CPU build, and every stop exits 0, where the published CUDA build's CPU fallback crashed in 10 of 10 runs.
  - In WSL, `wsl-extra.sh hidden` takes the CPU build the same way, and `wsl-gateway.sh`'s `auto` boots still name `b4938-linux-x86_64-cuda`, which runs the C++ runtime reader against Ubuntu 24.04's `libstdc++.so.6`.
  - As the last step of its component, it runs the full suite.

### Step 15: Cancel speech downloads with the boot command's token [completed]

- In `crates/gateway/local/src/artifacts.rs`:
  - A new `ArtifactStore::provision_whisper_library_with_cancellation(backend, activity, token: Option<&CancellationToken>)` selects the row as before and passes `token` to `provision_install`.
  - `provision_whisper_library(backend, activity)` is removed, because `prepare()` was its one production caller; `provision_whisper_library_reuses_a_verified_install` calls the new method with `None`.
  - The twin's doc says a fired token stops the download at its next chunk or the next phase boundary, never inside an extraction or the probe, and returns `LocalError::Cancelled`.
- In `crates/gateway/stt/api/src/artifacts.rs`:
  - `prepare(config, progress, cancel: &CancellationToken)` hands the token to `prepare_impl`, whose injected library provision gains an `Option<&CancellationToken>` parameter and is `ArtifactStore::provision_whisper_library_with_cancellation` in production.
  - `provision_models` gains the token and calls `ensure_model_with_cancellation` for each speech model.
  - A `LocalError::Cancelled` from either provision becomes `SpeechError::InitialLoadCancelled`, the error `SpeechService::load_initial` already documents for a cancellation before publication.
- In `crates/gateway/stt/api/src/generation.rs`, `GenerationState::load_initial` passes its `cancel` to `artifacts::prepare`.
- The boot command does not change. Its token already reaches `load_initial` through `load_speech` in `crates/gateway/app/src/boot_load.rs`, which reports any failure under a fired token as `GatewayError::CommandCancelled`.
- `SpeechService::load_initial`'s doc, in `crates/gateway/stt/api/src/service.rs`, adds that cancellation stops the whisper library and speech model downloads at their next chunk.
- Docs:
  - `guide/src/gateway/05-speech.md`, under "The runtime": a stop during the speech load cancels the whisper library and speech model downloads at their next chunk, the next start resumes them, and an extraction already under way finishes first.
  - The speech paragraph of `crates/gateway/app/README.md` says the same.
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
- Tests:
  - In `crates/gateway/local/src/artifacts/tests.rs`, `a_cancelled_whisper_provision_downloads_nothing`: with a fired token and an explicit backend, `provision_whisper_library_with_cancellation` returns `LocalError::Cancelled`, and the store holds no download and no install.
  - In `crates/gateway/stt/api/src/artifacts.rs`, `prepare_hands_the_load_token_to_the_library_provision`: the injected provision receives the load's token, and its `LocalError::Cancelled` reaches the caller as `SpeechError::InitialLoadCancelled`.
  - In the same file, `a_fired_token_stops_a_speech_model_download`: with the library provision injected, a pinned `https` model source under a fired token fails as `SpeechError::InitialLoadCancelled` without making a request.
  - `prepare_passes_the_stt_whisper_backend_to_the_library_provision` moves to the new closure.
- Checks:
  - `cargo test --locked -p gateway-local --lib artifacts::tests::`, `cargo test --locked -p gateway-stt --all-features --lib artifacts::tests::`, and `cargo test --locked -p gateway --lib boot_load::` pass.
  - Linux, `wsl-lifecycle2.sh` with `ONLY=download`: the stop during the throttled 743 MB download exits within a second with no `did not stop` or `did not retire` warning, where the end-to-end round's took 10 s and logged both, and the next boot resumes the partial download.

### Step 16: Carry the admission epoch's cancellation flag on every decode request [completed]

- In `crates/gateway/stt/engine/src/decoder.rs`:
  - `DecodeRequest` gains `cancellation: Option<Arc<AtomicBool>>`, set by `#[must_use] pub fn with_cancellation(self, flag: Arc<AtomicBool>) -> Self` and read by `pub fn cancellation(&self) -> Option<&Arc<AtomicBool>>`.
  - Their docs say the flag reads true once the caller has abandoned the decode, that a decoder may then stop early and fail, and that clones share it.
- `crates/gateway/stt/engine/src/worker.rs` does not change: the worker hands each request to `Decoder::decode` with its flag untouched.
- In `crates/gateway/stt/engine/src/test_fixtures/scenarios.rs`, `ScriptedDecoder` gains two controls:
  - `park_next_until_cancelled(bound: Duration)` makes its next decode poll that request's flag until it reads true or `bound` passes, and then fail;
  - `observed_cancellation()` reports whether a parked decode saw its flag set.
- In `crates/gateway/stt/api/src/admission.rs`, `EpochState.cancelled` becomes an `Arc<AtomicBool>`, and `SessionEpoch::cancellation_flag()` returns a clone of it. `cancel` and `is_cancelled` keep their Release and Acquire orderings.
- In `crates/gateway/stt/api/src/generation-lease.rs`, `GenerationLease::decode`, the one path every batch and Realtime decode takes, attaches the epoch's flag with `with_cancellation` before it hands the request to the runtime.
- `SpeechService::shutdown`'s doc, in `crates/gateway/stt/api/src/service.rs`, says that closing admission sets the flag every admitted decode carries.
- Tests:
  - In `decoder.rs`, `miri_a_cancellation_flag_reaches_every_clone`: a request's clone shares its flag, and a request built without one has none.
  - In `admission.rs`, `miri_shutdown_cancels_the_epoch_and_stops_admission` also asserts that a flag taken before the shutdown reads false until it and true after.
  - In `crates/gateway/stt/engine/src/test_fixtures/tests.rs`, `a_decode_parked_until_cancelled_follows_its_request_flag`: the parked decode returns once its request's flag is set, and fails at the bound for a request without one.
  - In `crates/gateway/stt/api/tests/it/generation.rs`, `shutdown_ends_a_decode_that_watches_its_cancellation_flag`: with a batch decode parked until cancelled under a 10 s bound, `SpeechService::shutdown` on a blocking thread returns within a second, and the decoder saw the flag. Without the flag from `GenerationLease::decode`, the shutdown waits out the park.
  - In the same file, `batch_decodes_carry_the_epoch_cancellation_flag`: a finished batch decode's captured request carries a flag that reads false, and true after `SpeechService::shutdown_admission`.
  - In `crates/gateway/stt/api/tests/it/realtime_session.rs`, `realtime_interim_and_final_decodes_carry_a_cancellation_flag`: a scripted session's `run_interim` decode and its committed item's final decode each carry an unset flag.
- Checks:
  - `cargo test --locked -p gateway-stt-engine --all-features` and `cargo test --locked -p gateway-stt --all-features` pass.
  - The runner's drain tests that hold a bare worker job, run with `cargo test --locked -p gateway --lib drain_tests::`, still abandon the retirement at the bound, because a job that runs no decode has no flag to read.
  - CI's STT gate, `RUSTFLAGS="-D warnings" cargo rustc --locked -p <crate> --lib -- -F unsafe-code`, passes for `gateway-stt-engine` and `gateway-stt`.
  - CI's Miri jobs run the new `miri_` tests. Their pinned nightly is not installed on this machine.

### Step 17: Abort running whisper decodes when admission shuts down [completed]

- In `crates/gateway/stt/whisper-ffi/src/raw.rs`, a new `AbortCallback`, `Option<extern "C" fn(*mut c_void) -> bool>`, is `ggml_abort_callback` from the pinned ggml.h, and `FullParams.abort_callback` takes that type in place of `*mut c_void`. A nullable function pointer is pointer-sized, so `pinned_b4938_parameter_layout_matches_the_64_bit_c_abi` still holds at 304 bytes.
- In `crates/gateway/stt/whisper-ffi/src/params.rs`:
  - `FullParams` gains `abort_flag: Option<Arc<AtomicBool>>` and `pub fn set_abort_flag(&mut self, flag: Arc<AtomicBool>)`. Its doc says whisper reads the flag after each encoder pass and decoder step, and that a set flag ends the pass with `WhisperError::Inference`.
  - A new `extern "C" fn abort_requested(data: *mut c_void) -> bool` loads the flag at `data` with Acquire ordering, answers false for a null pointer, and never panics, as `tracing_bridge` in `log.rs` is written. Its one unsafe read carries a `// SAFETY:` line naming the ownership below.
  - `FullParams::apply` writes `abort_requested` and `Arc::as_ptr` of the flag as its user data when a flag is set, and leaves whisper's null defaults otherwise.
- In `crates/gateway/stt/whisper-ffi/src/context.rs`, the `// SAFETY:` comment in `WhisperState::full` adds that the abort flag stays owned by `params`, which the call borrows throughout, and its `# Errors` section names the aborted pass.
- In `crates/gateway/stt/backend-whisper/src/model.rs`:
  - `WhisperDecoder::decode` passes `request.cancellation()` to `transcribe_blocking`, which hands the flag to `FullParams::set_abort_flag`.
  - `transcribe_blocking` returns an inference error at once when the flag already reads true, so a decode still queued at the stop runs no encoder pass. whisper first reads the flag after an encoder pass, and on the CPU build one `small.en` pass takes seconds per queued request, enough to outlast the shared deadline.
  - An aborted pass surfaces through `inference_error` as the existing `TranscribeError::Inference`, and its request fails.
- In `crates/gateway/app/src/runner.rs`, the shutdown paragraph of `Gateway::serve` adds that closing admission aborts running decodes after their current encoder pass or decoder step.
- Docs, including the second residual in the Deferred list:
  - `guide/src/gateway/05-speech.md`, under "The runtime": a graceful stop aborts running transcriptions after their current encoder pass or decoder step, their requests fail, and speech retires within the existing shutdown bounds.
  - The same paragraph says that a stop while a speech model is still loading can outlast those bounds, and the gateway then exits without retiring speech.
  - The speech paragraph of `crates/gateway/app/README.md` says the same.
  - `guide/promptforge-gateway-guide.md`, regenerated with `cargo run -p build-user-guide`.
- Tests:
  - In `params.rs`, `abort_requested` reads false for null data and for an unset flag, and true once the flag is set. With a flag set, `apply` writes the callback and the flag's address, and without one it leaves both null.
  - `crates/gateway/stt/backend-whisper/tests/native_whisper.rs` gains `a_decode_ends_when_its_cancellation_flag_is_set`, through an `SttEngine`. It is the native check of the FFI's abort path too, so `gateway-whisper-ffi` gains no native test and no dev-dependency:
    - a final decode of the clip repeated to several minutes fails with `TranscribeError::Inference` within a second of its flag being set mid-pass;
    - a decode whose flag is already set fails at once, and the clip with its flag unset transcribes;
    - like the suite's other tests, it ends its speech engine with `SttEngine::shutdown()`.
- Checks:
  - `cargo test --locked -p gateway-whisper-ffi --lib` and `cargo test --locked -p gateway-stt-backend-whisper --lib` pass, as do CI's STT gates for the two crates: `RUSTFLAGS="-D warnings" cargo check --locked -p gateway-whisper-ffi --lib`, and the `-F unsafe-code` build of `gateway-stt-backend-whisper`.
  - The ignored native suites `cargo test --locked -p gateway-whisper-ffi --lib -- --ignored --test-threads=1` and `cargo test --locked -p gateway-stt-backend-whisper --test native_whisper -- --ignored --test-threads=1` pass on four builds, with the model and audio in `vibe/scratch/stt-native/`:
    - run 36900875610's Linux CUDA archive and the pinned Linux CPU build, in WSL;
    - run 36900875610's Windows CPU archive and the pinned Windows CUDA build, on this Windows machine.
  - Linux, `wsl-longaudio.sh`: ten stops, by `POST /shutdown` and SIGINT, with six 11-minute requests in flight, exit 0 with no `CUDA error` and no `speech did not retire` warning, where the end-to-end round recorded one `CUDA error` and one exit 139 in four such stops.
  - Windows, `win_gateway.py long` shows the same on the CUDA and CPU builds.
  - As the final step, it runs the full suite: every exit gate in the Testing Plan, run on this machine as its last bullet describes.
