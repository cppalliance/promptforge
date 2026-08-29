---
name: CUDA llama provisioning
overview: Fix the remaining digest-marker defect, extract the tool, transcription, and desktop-shell boundaries, compile a host-native CUDA llama-server during the Cargo build, add generic MTP and multimodal companions, then safely replace the installed dense Gemma profile with Unsloth's fast E2B profile.
todos:
  - id: commit-runtime-rule
    content: Commit the authorized root AGENTS.md runtime-compilation rule as Step 0
    status: pending
  - id: marker-write-fallback
    content: Make both digest-marker persistence paths best-effort
    status: pending
  - id: extract-tools
    content: Extract the tool contract and move web-search ownership behind it
    status: pending
  - id: extract-transcribe
    content: Extract the Whisper transcription engine into promptforge-transcribe
    status: pending
  - id: extract-desktop-shell
    content: Extract the desktop window and WebView bridge into promptforge-desktop-shell
    status: pending
  - id: cuda-build
    content: Produce an embedded host-native CUDA llama-server bundle during Cargo build
    status: pending
  - id: cuda-stage
    content: Verify and atomically stage the embedded CUDA bundle at runtime
    status: pending
  - id: companion-config
    content: Add typed speculative and multimodal companion configuration
    status: pending
  - id: companion-runtime
    content: Provision companion artifacts and preserve their launch arguments across respawn
    status: pending
  - id: live-integration
    content: Prove and document CUDA, MTP, multimodal, and cache behavior
    status: pending
  - id: profile-migration
    content: Roll out E2B safely, then permanently remove the cached 31B model
    status: pending
isProject: false
---

# CUDA llama.cpp provisioning

## 0. Context handoff
- No code implementation from this plan has started. All todos are pending.
- One authorized planning-time repository edit exists: `C:\Users\Vinnie\cursor\promptforge\AGENTS.md` now states that runtime and serve paths never compile native dependencies or invoke build tools. Step 0 commits this exact edit before the normal clean-worktree gate. Do not discard or stash it.
- The operator selected all three preparatory extractions: tool contract and web search first, then transcription, then desktop shell.
- The operator requires native dependencies to compile during Cargo build only. PromptForge runtime and serve paths must never invoke compilers, CMake, Git, PowerShell, or another build tool.
- llama.cpp source will be an exact pinned git submodule. Developer builds target GPUs visible on the build machine. A future installer will use the same versioned bundle format with a portable release architecture profile; installer packaging itself is outside this plan.
- The operator selected exact Unsloth behavior: Gemma 4 E2B UD-Q4_K_XL, root Q8 MTP drafter, draft maximum two, F16 projector, CUDA, and removal of the old 31B cache only after successful replacement verification.
- The operator overrides the Vibe default that allows non-Critical findings to carry forward. Each step gets exactly one Review-and-Fix invocation. That invocation must fix every finding of every severity and leave zero open findings, or return blocked and stop the run. Do not run a second review over the reviewer's fixes.
- Execute autonomously under the loaded Vibe and Rust rulebooks. Make reversible decisions without asking. Stop only for an expensive-to-reverse ambiguity, an unfixed Critical finding, a dirty initial worktree, or no forward path.

## 1. What this builds
The repository root is `C:\Users\Vinnie\cursor\promptforge`. A clean Windows x86-64 Workshop build with its default `cuda` feature will compile a pinned llama.cpp submodule for the GPUs visible on the build machine. Cargo performs all source compilation. The gateway binary embeds the resulting runtime manifest and files, then only verifies and atomically stages them into its cache when it runs.

Runtime and serve paths will never invoke CMake, NVCC, MSBuild, Git, PowerShell, or another build tool. The policy already exists in [C:\Users\Vinnie\cursor\promptforge\AGENTS.md](C:\Users\Vinnie\cursor\promptforge\AGENTS.md) and is committed by Step 0. Builds without `llama-cuda` keep the current Vulkan or Metal archive path.

Before the CUDA work, three existing boundaries will become crates: the core tool contract with a separate web-search provider, the Whisper transcription engine, and the desktop shell. Public re-exports and compatibility feature aliases preserve current callers.

Local model configuration will then support an explicit MTP draft artifact and multimodal projector. These artifacts use the existing source, pin, marker, lock, and cache machinery. The final operational rollout replaces dense Gemma 4 31B with Unsloth's Gemma 4 E2B UD-Q4_K_XL, its Q8 MTP drafter, and its F16 projector.

## 2. High-level components
1. **Artifact verification.** Marker persistence becomes a non-fatal optimization after successful hashing.
2. **Tool surface.** Tool contracts and catalogs move out of core, then concrete web search moves into its own provider crate.
3. **Transcription engine.** Whisper model ownership, inference workers, segmentation, and CUDA implementation move out of the Workshop HTTP server.
4. **Desktop shell.** Tao, wry, icons, IPC, and the Windows WebView2 bridge move behind one desktop-shell API.
5. **CUDA native bundle.** Cargo builds, manifests, embeds, verifies, and stages a host-native CUDA `llama-server`.
6. **Local model companions.** Typed configuration and runtime wiring provision MTP and projector artifacts and preserve their launch state.

Integration, benchmarking, and installed-profile migration verify and deploy these components. They are not separate software components.

## 3. Component pieces and dependency order
- **Artifact verification, sequential:** repair both marker-write call paths, test them, then run mandatory verification because this closes the inherited artifact finding.
- **Tool surface, sequential across commits:** first extract stable tool vocabulary and preserve core re-exports; then move the concrete web-search implementation into a provider crate that depends on the new contract. The provider move depends on the extracted contract.
- **Transcription engine, atomic:** move the cohesive engine and its tests in one commit while leaving voice WebSocket routes and activation glue in `promptforge-ws-server`.
- **Desktop shell, atomic:** move windowing and the Windows bridge together so the unsafe boundary and heavy GUI dependencies leave `promptforge-ws` without an intermediate broken build.
- **CUDA native bundle, sequential across commits:** first produce a deterministic embedded bundle during Cargo build; then consume that bundle in runtime staging. The second piece depends on the generated manifest contract from the first.
- **Local model companions, sequential across commits:** first add typed public configuration and validation; then provision the declared artifacts and emit launch arguments. Runtime wiring depends on validated configuration types.
- Pure command-plan, fingerprint, manifest, and argument helpers inside a commit may be developed in parallel. Numbered commits remain sequential.

## 4. Execution protocol
This is a Full-path run under [C:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md](C:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md) and [C:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md](C:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md).

Before Step 0:
- Inspect `C:\Users\Vinnie\cursor\promptforge`. Permit only the exact authorized `AGENTS.md` diff described in the context handoff. If any other tracked, staged, or untracked change exists, stop and report it.

After Step 0 and before Step 1:
- Require a clean worktree.
- Create scratch files under `cabinet/_scratch/vibe-cuda-llama-provisioning/`: `vibe-ledger.md`, `vibe-review.md`, and `rules-manifest.md`.
- Initialize `vibe-review.md` with the previous Minor finding verbatim: marker-write I/O failure fails an otherwise successful verification instead of degrading to a re-hash next time. Record both call paths, `verified.rs:78` and the post-download call in `artifacts.rs`.
- Survey the root and nested `AGENTS.md` files and write their governed directories to `rules-manifest.md`.
- Ensure `cargo-public-api` is available and record its version. It is required for the crate-extraction and public-configuration API snapshots.
- Resolve tag `b10082` to an exact llama.cpp commit and confirm from that source that the required draft-MTP arguments and Windows CUDA build options exist. If either check fails, stop and re-plan before any implementation commit.

Step 0 is an authorized rule-only commit and therefore has no Coder or code test. Every numbered implementation step after it is one commit with code and tests. Run Coder, Message, one Review-and-Fix, Amend when needed, and scheduled Verify. The following plan-local review contract supersedes the Vibe carry-forward and second-pass behavior:
- The single Review-and-Fix invocation reviews the complete step diff reducers-first in this order: delete, narrow, deduplicate, reshape, fix, add.
- It fixes every finding from the complete diff in the same invocation, including Minor findings, and reruns the focused tests when it changes code or tests.
- It may reject a finding only with source-confirmable evidence. It may not defer or carry a finding to a later step.
- It writes `vibe-review.md` with zero open findings before returning. If it cannot do so, it returns blocked and the run stops for re-planning.
- No second review invocation or fix-diff review occurs. Scheduled Verify remains an independent fresh challenger and gates the next step.
- A Verify failure returns the complete failure log to the Coder for one comprehensive repair, followed by one re-Verify. If still red, stop instead of entering an incremental fix-forward loop.

Borrow these techniques from `tools-public/tools/refactor-rust.md` without adding subcontexts:
- For Steps 2 through 5 and Step 8, the Coder captures pre-edit `cargo public-api` snapshots for affected library crates, records intended additions, removals, and compatibility re-exports from real call sites, and writes them to scratch.
- The existing Review-and-Fix checks actual public surfaces against that intent and checks every tracked in-repository consumer migration.
- The existing Verify role regenerates candidate snapshots and fails on unplanned API drift.
- Review and Verify scan added non-test library lines for dead-code allowances, ignored rustdoc fences, contradictory test cfgs, production `unwrap` or `expect`, secret-bearing debug output, and security-sensitive random values.
- Do not adopt refactor-rust candidate branches, per-file analyzer fan-out, repeated fixer rounds, discard-and-restart loop, or additional challenger contexts.

## 5. Commit rationale map
The Message role uses the matching entry below only when the staged diff proves that step occurred. It writes the rationale in plain language, does not mention the plan or step number, and does not claim behavior outside the diff.

0. **Runtime build boundary:** Native compilation inside a running gateway would make ordinary startup depend on developer toolchains, permit long and failure-prone build processes inside the serving lifecycle, and complicate installer security. The root rule places compilation in Cargo or packaging while allowing runtime to verify, stage, and launch artifacts produced there.
1. **Marker persistence:** Digest verification protects model integrity; the marker only avoids repeating that expensive verification. A failure to save an optimization record must not convert already-verified model bytes into a profile-switch failure. Confinement and digest mismatch remain hard errors because they protect trust boundaries rather than performance.
2. **Tool contract:** The future addon host must adapt proprietary DLL descriptors into `Tool` objects without depending on the parser, Lua runtime, executor, HTTP clients, or the rest of core. A small runtime-agnostic contract crate gives built-in tools, web providers, core, and the addon host one stable Rust-side vocabulary while keeping the C ABI independent.
3. **Web-search provider:** `WebSearch` performs network I/O and owns credentials, deadlines, and response decoding, so it does not belong in either the pure tool contract or execution core. A provider crate preserves a narrow contract crate, isolates HTTP dependencies, and gives secret handling one review boundary.
4. **Transcription engine:** Whisper inference is a compute subsystem with worker ownership and an independent CUDA dependency. Moving it out of the Workshop HTTP server prevents voice CUDA from being confused with llama CUDA, reduces server dependency weight, and leaves WebSocket code responsible only for session transport and activation.
5. **Desktop shell:** Windowing and the Windows WebView2 bridge are platform infrastructure, not application lifecycle orchestration. Their extraction isolates heavy GUI dependencies and the workspace's exceptional unsafe code, restores normal lints to the desktop binary, and creates the packaging boundary a future installer will consume.
6. **CUDA build bundle:** PromptForge currently hard-codes a Vulkan llama.cpp archive on Windows even for NVIDIA systems. Compiling the pinned source during Cargo build produces a backend optimized for the developer's GPU while enforcing the project rule that a running gateway never invokes compilers or build tools. A pinned submodule keeps source identity reviewable and prevents a build-time network resolver from silently changing inputs.
7. **Runtime bundle staging:** Cargo build output is not itself a stable runtime installation and does not automatically survive packaging. A versioned embedded manifest plus atomic staging gives development and future installer builds one verifiable bundle contract, detects tampering or dependency drift, and lets the gateway reuse its existing private-cache guarantees without compiling anything.
8. **Companion configuration:** Unsloth's fast Gemma path requires an MTP drafter and multimodal projector in addition to the main GGUF. Explicit generic companion configuration makes those artifacts reproducible and pinned without hard-coding Gemma repository-name heuristics or coupling configuration parsing to network discovery.
9. **Companion runtime:** Declarative companion fields have no value unless every launch and respawn receives the same verified paths and speculation arguments. Reusing `ensure_model` gives main, draft, and projector artifacts identical integrity and cache behavior while owned launch state prevents respawn drift.
10. **Live proof and documentation:** CUDA linkage, GPU offload, MTP acceptance, projector use, and multi-gigabyte cache reuse can all compile successfully yet fail only on real hardware. One opt-in live path proves the complete behavior on the target host, while documentation makes the build and configuration contracts reproducible for another developer.

The installed-profile rollout has a separate operational rationale: E2B plus MTP replaces the dense 31B model to obtain the observed interactive speed, but the old catalog and model remain available until the replacement proves text, tools, vision, CUDA, and speculative decoding. Permanent cache deletion occurs last so reclaiming disk space never removes the rollback path.

## 6. Numbered implementation steps
0. **Commit the runtime-compilation rule.** Confirm the only worktree change is the authorized addition to [AGENTS.md](AGENTS.md): runtime and serve paths never compile native dependencies or invoke compilers or build tools; native compilation belongs to Cargo build or packaging, and runtime may only verify, stage, and launch build-produced artifacts. Stage only `AGENTS.md`, dispatch the Message role using rationale entry 0, commit, run the single Review-and-Fix pass over that commit, amend only if the review changes the rule text, and confirm the worktree is clean. No Coder or Cargo verification is required because this step changes repository guidance only.

1. **Make marker persistence best-effort.** Change [crates/promptforge-gateway/src/local/artifacts/verified.rs](crates/promptforge-gateway/src/local/artifacts/verified.rs) and [crates/promptforge-gateway/src/local/artifacts.rs](crates/promptforge-gateway/src/local/artifacts.rs) so a validated path escape or digest mismatch remains an error, but failure to persist a marker after a successful hash is handled once with a stated tracing reason and returns success. Add deterministic tests in [crates/promptforge-gateway/src/local/artifacts/tests.rs](crates/promptforge-gateway/src/local/artifacts/tests.rs) for both verification and post-download marker failures. This is the separate fix for the defect introduced by commit `7b4e2ae`. Run mandatory Verify with the gateway build and focused artifact tests because this closes the artifact component.

2. **Extract the tool contract.** Add publishable `promptforge-tools` containing `Tool`, `ToolCatalog`, `ToolId`, shared tool inputs and outputs, and contract errors from `promptforge-core`. Add `crates/promptforge-tools/AGENTS.md` with the binding rule that this crate contains runtime-agnostic tool vocabulary only and never depends on HTTP clients, concrete providers, Lua, parser, executor, gateway, or core. Add `crates/promptforge-core/AGENTS.md` stating that core owns parsing and execution while tool contracts and concrete providers remain outside it; compatibility re-exports are allowed. Add or update `crates/promptforge-webfetch/AGENTS.md` to keep that crate limited to fetching and converting a known URL and dependent on `promptforge-tools`, not core. Update `promptforge-webfetch` and every direct consumer to depend on the new crate for tool vocabulary. Keep compatibility re-exports under `promptforge_core::tools`, preserve public names and behavior, document every public item, and move existing contract tests with regression coverage for catalog identity and dynamic dispatch. Leave the concrete `WebSearch` implementation in core for this commit. No Verify is scheduled unless review changes the tree.

3. **Extract the web-search tool.** Add publishable `promptforge-web-search` containing the concrete `WebSearch` implementation and its HTTP tests from `promptforge-core`. Add `crates/promptforge-web-search/AGENTS.md` requiring dependence on `promptforge-tools` rather than core or gateway, provider-only ownership, source-preserving errors, bounded requests, and secret-free diagnostics. Update CLI, MCP server, development tooling, and core compatibility re-exports without creating a dependency cycle. Keep `promptforge-webfetch` focused on fetching and converting a known URL. Prove constructor validation, secret-safe debugging, request deadlines, response decoding, and `Tool` conformance with the moved tests. Run mandatory Verify because this is Step 3 and completes the tool-surface component.

4. **Extract the transcription engine.** Add `promptforge-transcribe` as a workspace crate containing `VoiceEngine`, `VoiceSlot`, worker ownership, segmentation, silence gating, Whisper integration, and their focused fixtures and tests from [crates/promptforge-ws-server/src/transcribe](crates/promptforge-ws-server/src/transcribe) and `segment.rs`. Add `crates/promptforge-transcribe/AGENTS.md` requiring engine-only ownership, no HTTP, WebSocket, Workshop server, gateway, or UI dependencies, and additive CUDA behavior. Append the matching boundary to the existing `promptforge-ws-server/AGENTS.md`. Give the new crate a `cuda` feature that alone forwards to `whisper-rs/cuda`. Keep HTTP, WebSocket voice-session handling, route state, and post-cache activation in `promptforge-ws-server`; map its `VoiceConfig` into a narrow transcription constructor so the new crate never depends back on the server. Add a `voice-cuda` server feature and retain `cuda` as a compatibility alias. Preserve all public behavior and test fixtures. Run mandatory Verify because this completes the transcription component.

5. **Extract the desktop shell.** Add unpublished `promptforge-desktop-shell` containing [crates/promptforge-ws/src/window.rs](crates/promptforge-ws/src/window.rs), `file_drop.rs`, icon assets, tao/wry event-loop ownership, IPC, navigation policy, microphone permission, and the Windows WebView2 bridge. Add `crates/promptforge-desktop-shell/AGENTS.md` limiting the crate to windowing, WebView, IPC, and platform bridges while confining unsafe code to the documented Windows bridge. Append to the existing `promptforge-ws/AGENTS.md` that the desktop binary remains lifecycle orchestration and does not reacquire GUI implementation dependencies. Expose one narrow documented `run` entry point. Move GUI and Windows COM dependencies out of `promptforge-ws`, isolate the existing unsafe allowance in the new platform crate, and restore workspace lints to `promptforge-ws`. Keep configuration discovery, gateway start, health wait, shutdown, and feature forwarding in the desktop binary. Move existing tests without weakening assertions and add a caller-boundary test. Run mandatory Verify because this completes the desktop-shell component.

6. **Build and embed the CUDA bundle.** Add the pinned `https://github.com/ggml-org/llama.cpp.git` submodule at `third_party/llama.cpp`, fixed to the exact commit behind `b10082`. Add `crates/promptforge-gateway/AGENTS.md` that specializes the existing root rule for gateway code: runtime may only verify, stage, and launch build-produced native bundles. Add the additive `llama-cuda` feature to [crates/promptforge-gateway/Cargo.toml](crates/promptforge-gateway/Cargo.toml), make `workshop-cuda` imply both `llama-cuda` and `promptforge-ws-server/voice-cuda`, and preserve the `promptforge-ws` feature forwarding. Add a gateway `build.rs` and cohesive build-support modules, or a small private build-support crate if tests require it. When `CARGO_FEATURE_LLAMA_CUDA` is present on Windows x86-64, use Cargo target variables rather than Rust host constants, reject cross-compilation, require the local CUDA Toolkit, detect all visible local GPU compute capabilities, and compile only those architectures. For this host, CUDA 12.8 or newer and Blackwell compute capability 12.0a are required. Configure a Release server build with CUDA enabled, project libraries static, tests and unrelated upstream programs disabled, and every material CMake option explicit. Build scripts read the submodule and write only under `OUT_DIR`.

   Generate a versioned canonical manifest containing the submodule commit, source identity, target triple, MSVC and CMake identities, resolved NVCC path and version, toolkit version, normalized architecture list, full material option set, linkage policy, bundle format version, and SHA-256 for every runtime file. Account for the complete PE dependency closure: bundle all llama.cpp or GGML runtime files, keep Windows system and declared CUDA Toolkit DLLs external, record their required names, and require the same compatible toolkit at runtime. When host equals target, smoke-check the staged executable with its device-list operation and require a CUDA device. Generate Rust source under `OUT_DIR` that embeds the manifest and each runtime file with `include_bytes!`.

   On targets other than Windows x86-64, `llama-cuda` adds no llama backend and the existing platform backend remains unchanged. Build-support tests use temporary directories and injected command probes. They cover target selection, local architecture normalization, canonical identity, exact command plans, submodule absence or drift, bounded command failure output, dependency allowlists, manifest generation, and synthetic Windows runtime trees without invoking real CMake or accessing the network. Run mandatory Verify because this is Step 6.

7. **Stage the embedded CUDA bundle at runtime.** Add a focused runtime module under `local/artifacts/` that consumes only the generated embedded manifest and bytes. Validate manifest schema, filenames, digests, target, toolkit dependency availability, and cache confinement before use. Publish through the existing advisory lock, private staging directory, tree digest, install marker, and atomic rename. A valid matching installation returns immediately. A CUDA-enabled Windows build never silently selects Vulkan. Keep build-script failures private to build support; represent runtime extraction and validation failures with a narrow error wrapped by `LocalError`, with no panic and no duplicate logging. Prepend the staged directory and compatible CUDA Toolkit runtime directory to the child environment without mutating process-global state. Tests cover cache hit, tampering, target mismatch, missing toolkit dependency, interrupted staging, concurrent publication, cleanup, and child environment construction. Run mandatory Verify because this closes the CUDA bundle component.

8. **Add typed companion configuration.** Add `crates/promptforge-gateway-config/AGENTS.md` requiring declarative configuration only, no network or process execution, secret-safe diagnostics, and validation before values leave the crate. Add cohesive local-model companion types in a new gateway-config module rather than expanding the existing large accessor file. Define documented, non-exhaustive public types for `SpeculationType`, `SpeculativeConfig`, `MultimodalProjectorConfig`, and a bounded nonzero draft-token count. Initially support only serialized `draft-mtp`. The MTP form requires a source, digest when remote, and draft-token maximum in the supported llama.cpp range. Projector configuration requires a source and digest when remote. Both are chat-only and reject empty or insecure sources. Re-export the types through the existing facade and provide documented accessors and compiled examples. Tests cover valid parsing, unknown speculation types, every invalid cross-field combination, HTTP rejection, missing remote pins, path sources, defaults, and whole-entry profile replacement. No Verify is scheduled unless review changes the tree.

9. **Provision and launch companions.** In [crates/promptforge-gateway/src/local/mod.rs](crates/promptforge-gateway/src/local/mod.rs), resolve the MTP drafter and projector independently through the existing `ensure_model` path. Store owned resolved paths in `LaunchOptions` so respawn needs no new external state. Extend [crates/promptforge-gateway/src/local/server/support.rs](crates/promptforge-gateway/src/local/server/support.rs) to emit the exact pinned-server arguments for draft model, `draft-mtp`, maximum draft tokens, and multimodal projector. Preserve omission for models without companions. Tests cover independent cache slots and pins, argument order and omission, initial launch, respawn parity, shutdown, and failures before child spawn. Run mandatory Verify because this is Step 9 and closes the companion runtime component.

10. **Add live proof and documentation.** Add a feature-gated ignored integration test in the existing single integration-test binary. Its ignore reason must name the Windows CUDA Toolkit, NVIDIA GPU, model downloads, and required opt-in environment variable. Run serially with explicit phase timeouts. Verify a clean CUDA build, embedded bundle extraction, a CUDA device report, target and draft GPU-layer offload, all three model digests, MTP response timings with positive drafted and accepted token counts, cache reuse, a tool call, and a real image-content completion through the projector. Update the affected crate READMEs and gateway and Workshop documentation for the new crate boundaries, submodule checkout, toolkit and target requirements, feature behavior, build-time compilation, runtime staging, companion syntax, non-CUDA behavior, and diagnostics. Run final Verify with the complete gates below.

## 7. Verification gates
The final Verify runs on this Windows x86-64 Blackwell host:
- Rust formatting check across the workspace.
- Clippy across all targets and all features with warnings denied.
- Locked workspace tests with all features.
- Doctests.
- Documentation with warnings denied and all features.
- Feature-power-set checks to depth two without development dependencies.
- The no-default-feature Workshop build.
- The opted-in ignored CUDA, MTP, and projector integration test.
- UI type checking and tests when UI files or its build path are affected.

The standard CI path must continue testing non-CUDA feature sets without requiring a CUDA Toolkit. A CUDA-capable scheduled or self-hosted job runs the native feature and ignored live test.

## 8. Reproducible performance check
After functional gates pass, record benchmark data as research:
- Compare E2B Vulkan without MTP, E2B CUDA without MTP, and E2B CUDA with MTP.
- Use the same UD-Q4_K_XL main file, 131072 context, one slot, flash attention, seed 42, deterministic sampling, and 1024 generated-token limit.
- Use one warm-up and three measured runs. Report medians for prompt speed, generation speed, first-token latency, total latency, drafted tokens, accepted tokens, and peak GPU memory.
- Use the fixed prompt: `Write the integers from 1 through 1000, separated by one space, and output nothing else.`
- Treat measurements as observations, not release thresholds. Functional acceptance requires CUDA device evidence and positive accepted MTP tokens in at least two measured MTP runs.

## 9. Safe installed-profile rollout
This is a post-Vibe operational procedure, not a repository commit:
1. Record the running Workshop executable and command, back up `C:\Users\Vinnie\.promptforge\gateway.toml` beside the original, and create a candidate catalog.
2. In the candidate, replace `gemma-4` with `https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-UD-Q4_K_XL.gguf`, pinned to `b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32`.
3. Configure the root drafter `https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mtp-gemma-4-E2B-it.gguf`, pinned to `9eba819938efccfd6044f8af84e3bbfddc639a2bcf32ebc36420e6a649191919`, with `draft-mtp` and maximum `2`.
4. Configure `https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf`, pinned to `140be8d7849741f88c50757d529b84373ee8e27052cc2236855b537f4a8215fa`. Keep model name `gemma-4`, profile `gemma`, one slot, flash attention, switchable thinking, and context `131072`.
5. Keep `C:\Users\Vinnie\.promptforge\profiles\gemma.toml` selecting `gemma-4`. Do not edit `D:\promptforge\gateway\gateway.toml`, which has no Gemma entry and is not active.
6. Validate the candidate configuration and pre-stage the three pinned model artifacts while retaining the old catalog and 31B cache.
7. Stop Workshop and every PromptForge-owned `llama-server`, atomically replace the catalog, restart the recorded executable with profile `gemma`, and run model-list, text, tool-call, image, CUDA, MTP, and digest checks.
8. On any failure, restore the catalog backup and restart the old profile. Do not delete the 31B cache.
9. Only after every replacement check passes, permanently delete `D:\promptforge\gateway\cache\models\7d0a74142ee9b0c8\gemma-4-31B-it-UD-Q4_K_XL.gguf`, its `.verified` marker, and the empty cache directory. The deletion is intentional and recoverable by downloading the pinned Hugging Face artifact. Leave Qwen models and lock files untouched.

## 10. Decisions and falsifiers
- **No runtime compilation:** repository policy. Cargo builds native dependencies; runtime only stages and verifies them. Falsifier: the project later accepts runtime compilation as a supported lifecycle.
- **Pinned submodule:** llama.cpp source lives at `third_party/llama.cpp` as the exact `b10082` commit. Falsifier: the pin lacks Blackwell CUDA or draft-MTP support, which requires re-planning the pin before coding.
- **Local GPU architecture:** build for GPUs visible on the build machine. Falsifier: Workshop binaries must be distributed to different NVIDIA generations.
- **Windows x86-64 scope:** CUDA llama support in this change is target-specific. Falsifier: Linux CUDA becomes a release requirement.
- **Toolkit runtime dependency:** project libraries are bundled, while declared CUDA Toolkit DLLs remain external on the same build-and-run host. Falsifier: Workshop gains portable binary distribution.
- **Explicit companions:** configuration names exact sources and pins instead of Hub sibling heuristics. Falsifier: operators need automatic companion discovery.
- **Fixed context:** rollout uses the observed 131072-token setting. Falsifier: startup, complete GPU offload, or the live test fails at that context.

## 11. Final plan review
- Step 0 is one rule-only commit. Each numbered implementation step after it is one code-and-test commit with one complete focused test set. Mandatory Verify runs at component boundaries, every third implementation step, after review changes, and on the final step.
- Data flow is complete: transcription receives server configuration through a narrow constructor; the desktop binary calls one shell entry point; core and provider crates consume the extracted tool contract through compatibility re-exports; the submodule and Cargo target inputs produce an embedded manifest and bytes; runtime staging validates and publishes that bundle; typed companion configuration produces validated artifact descriptions; runtime provisioning resolves owned paths into launch state; live tests consume the full path; rollout begins only after all code gates pass.
- Steps 6 and 8 may parallelize pure helper work internally. All commits and rollout actions remain dependency-ordered.
- The plan adds no runtime compiler, shell build path, automatic Hub discovery, auto-fit context, broad speculative policy, or unrelated provisioning framework.
- Confidence: high - the revised design satisfies the Vibe and Rust rulebooks, preserves rollback until replacement acceptance, and matches the selected source-built, local-GPU deployment model.

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: CUDA llama provisioning (cuda_llama_provisioning_8c797246)

## Origin: two problems converged

The plan grew out of a live incident, not a design exercise. While switching gateway profiles the operator's machine stalled hard: "when I switch profiles in the gateway the machine makes a big thud. cursor ui becomes unresponsive" - on a "96 core threadripper. 1TB RAM" with a "96gb blackwell". Diagnosis of that incident produced the digest-marker defect (a marker-write I/O failure failing an otherwise successful verification) that Step 1 closes, and the operator explicitly claimed both fixes: "I want the digest-marker and I also want the lowered priority can we do that?"

Days later the second problem surfaced. The operator compared Unsloth's Gemma 4 throughput against PromptForge's and asked: "Why is Gemma4 on Unsloth so fast, but in my app so slow?" Investigation found the gateway hard-codes a Vulkan llama.cpp asset as the only Windows x86-64 choice - a cross-vendor portability shortcut that was the wrong default for an NVIDIA host. The operator's reaction was the real requirement: "why the fuck am I using a Vulkan llama.cpp build?"

## The decisive design exchange: where compilation lives

The operator's first instinct was on-demand compilation: "why cant we just compile llama.cpp as needed". The assistant initially recommended the opposite - pinned prebuilt CUDA binaries first, cached local compilation only as fallback, and explicitly "a dedicated build/provisioning step over Cargo `build.rs`" (paraphrase). Both instincts were discarded. The operator then stated the toolchain contract that reshaped the plan: "I have the CUDA Toolkit. Anyone who builds promptforge-workshop is expected to have the toolkit if they want CUDA".

The pivotal moment was a clarifying question, quoted verbatim because it is the entire design in one line: "you are telling me that PromptForge will compile the llama-server? Do you mean this will happen during "cargo build" or do you mean that the promptforge-gateway is going to call into the shell and compile something?" The resolution - Cargo build-time compilation only, runtime never invokes build tools - was then elevated by the operator from plan detail to permanent repository policy: "add it to the root AGENTS.md" and "commit the AGENTS.md as the first step of the plan! step 0 !" That is why Step 0 exists and why the run must not discard the pre-staged AGENTS.md edit.

## Discarded alternatives

- **Runtime compilation by the gateway** (the operator's own first proposal): rejected once the cargo-build-vs-shell distinction was drawn; codified as the AGENTS.md prohibition.
- **Pinned prebuilt CUDA binaries with compile-as-fallback** (assistant's first recommendation): rejected as mismatched with the source-built deployment model once the toolkit expectation was stated.
- **Dedicated provisioning step outside Cargo** (assistant's early preference over `build.rs`): discarded; final design compiles inside the Cargo build from the submodule.
- **Fetching llama.cpp source over the network during build**: discarded for an exact pinned git submodule, keeping Cargo builds network-free.
- **Portable fat binaries covering multiple NVIDIA generations for developer builds**: deferred, not rejected. The operator's questions - "Okay but what happens when we want to build PromptForge as a binary with installer?" and "what happens when one of my team members builds on windows and they have a regular rtx 4090 instead of a blackwell" - produced the two-profile answer: developer builds target GPUs visible on the build machine; a future installer reuses the same versioned bundle format with a portable architecture list. Installer packaging was deliberately scoped out, with falsifiers recording the trigger conditions.
- **Silent Vulkan fallback**: explicitly banned ("A CUDA-enabled Windows build never silently selects Vulkan") because silent downgrade is exactly how the original defect shipped.
- **Vibe fix-forward review**: the operator overrode the rulebook default: "i dont want fix-forward I want fix-everything (but just one review pass)" - hence the plan-local single Review-and-Fix contract with zero open findings and no second pass.

## Why the E2B profile and companions

The model replacement was not a benchmark-driven optimization; it was "match what Unsloth runs." The operator directed it directly: "remove the gemma4 profile and entry from the gateway config ... and then add the fast gemma4 profile that unsloth uses", including the cache deletion: "yeah remove the model file as well (the multi-GB cached file)". The MTP drafter, draft-max 2, and F16 projector values were copied from observed Unsloth behavior, and explicit companion configuration was chosen over Hub sibling heuristics so every artifact carries a source and pin. The 31B cache survives until replacement verification passes because the operator wanted rollback, and the final deletion is intentional and recoverable by re-download.

## Why the three crate extractions precede the CUDA work

The operator commissioned a survey ("spawn 4 subagents and explore ... see if you can find big pieces that deserve to be in separate crates") and selected tool contract/web-search, transcription, and desktop shell as preparatory, informed by the eventual addon DLL ABI plan. The extractions are boundary work, not features: they keep the unsafe windowing code, heavy GUI dependencies, and engine code out of the crates the CUDA change touches, and per-crate AGENTS.md files were the operator's idea ("should you put AGENTS.md in the root of the crates that are being changed ... with rules?").

The operator also set the documentation bar for commits: "enrich the plan to include rationale so that the commit messages have enough information to answer "why"."

## Run-chat deviations (what actually happened vs the plan)

- **The pin falsifier fired sideways.** Benchmarks showed a ~120x CUDA deficit with 72 graph splits. The expected cause was missing gemma4 CUDA kernels at pin b10082. The actual root cause was configuration: the gateway's mismatched KV cache quant defaults (`cache-type-k q8_0`, `cache-type-v q4_0`) are rejected by the CUDA flash-attention kernel unless built with `GGML_CUDA_FA_ALL_QUANTS=ON`, forcing FLASH_ATTN_EXT onto CPU on all 35 layers. Fix: add that CMake flag - no re-pin required for the splits. A re-pin to b10680 still happened opportunistically: GET_ROWS on k-quant embeddings was fixed upstream at b10089, and a draft-mtp+embeddings fix landed at b10577, both relevant to this configuration.
- **Process deviation:** a long benchmark was run blocking, drawing "ALWAYS ASYNC SUBAGENTS". Treat all long-running benchmarks and builds as async subagent work.
- **CI deviation:** the plan's "scheduled or self-hosted job" became concrete during the run - the operator installed and authenticated `gh` and stood up a self-hosted runner on the Blackwell host; post-run, the nightly cron in `cuda.yml` was uncommented and set to 2 AM Pacific.
- **Rollout friction:** a stale gateway process survived its terminal and held ports 8081/7910, prompting a SO_REUSEADDR question; the answer is that Tokio deliberately omits it on Windows (where the flag permits port hijacking rather than TIME_WAIT rebind). The agent also overstepped by relaunching the gateway after being asked only to kill the stale process ("no I did not tell you to relaunch the gateway").
- **Post-rollout desktop-shell defects** surfaced immediately and were fixed outside the plan's commits: `open_browser = true` in the preserved config was honored alongside the desktop window (fixed in config; the flag is documented for headless use), and the web UI's custom window menu lacked rollover and focus-loss unpop behavior.
- **Outcome confirmation:** "Gemma4 runs beautifully now" - E2B on CUDA with MTP delivered the Unsloth-class behavior that motivated the plan.
