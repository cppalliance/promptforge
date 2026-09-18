# Dependency Surface

Report type: specification / measured record. It defines how the PromptForge dependency graph is measured, records what the measurements say at a named commit, lists what was removed or replaced and why, names what is deliberately kept, and states the invariants a future change must preserve. It answers "can we drop or reimplement X?" from measurement rather than intuition, and it is the dependency counterpart to the version and feature choices already commented inline in the root `Cargo.toml`.

- Measured at: the commit landing the `toml 1` pin (parent `26b29a7c`, which landed the `reqwest 0.13` pin), 2026-09-18; first measured at `92620c7e` the same day
- Toolchain: cargo 1.98.0
- Scope: the whole workspace (47 members), then the three shipped shapes: the gateway binary, the Workshop desktop binary, and the one-door library pair

## Abstract

PromptForge ships three binaries from one lockfile: `promptforge-gateway` (the credential boundary and the only process with an edge to an LLM backend), `workshop` (the Tauri desktop app, which embeds the executor), and the library pair `promptforge-api-runtime` and `promptforge-api-types` that a downstream host depends on. The lockfile holds 919 packages, but no single binary ships that many, and the number that matters for any one target is the normal-edge closure of that target on that platform. This document records those closures, ranks every direct third-party dependency by what it costs exclusively, and writes down the four measurement traps that produced wrong answers while it was being written so they are not repeated.

Three findings came out of the first measurement. The first, that the gateway binary linked both rustls crypto backends, is resolved and recorded under "What was replaced, and why". Two remain open at the end: `workspace-hack` places a 372-package floor under every member build including the types crate, and `turso` is the single largest exclusive cost in the tree at 59 crates for three plain tables.

## Method

Candidates are ranked by **exclusive transitive cost**: the crates that leave the graph entirely if a dependency goes, computed as the normal-edge closure of that dependency minus the normal-edge closure of every other direct third-party dependency of every workspace member. A crate that is shared with the rest of the tree costs nothing to keep, however large it looks alone. The second axis is **call-site surface**: how many distinct APIs the workspace actually touches, which is what a reimplementation has to reproduce. Call-site counts are not recorded in this revision; a removal proposal computes them at proposal time.

The measurement is taken from `cargo metadata --format-version 1 --locked --all-features --filter-platform <triple>`, walking the `resolve` graph over normal edges only, once per shipping platform. Per-binary shipped counts are taken with `cargo tree -p <package> -e normal --prefix none | sort -u`. Both are run at the named commit, and every number below states which platform it was taken on.

### Measurement traps

Four wrong answers were produced and caught while writing this. Each is easy to repeat.

1. **Unfiltered `cargo metadata` resolves every target at once.** The first pass reported 875 third-party packages and put `turso` at 77 exclusive crates. Filtered to `x86_64-pc-windows-msvc` the tree is 642 packages and `turso` is 59. The unfiltered graph contains the Linux GTK stack (`glib-macros`, which is the only thing still pulling `proc-macro-error` and `syn 1`), the macOS `swift-rs` (the only puller of `base64 0.21`), and `shuttle`, a deterministic concurrency scheduler that `turso_core` names for some target and that no shipping platform resolves. Always pass `--filter-platform`, and when a crate appears in the lock but `cargo tree -i` says "nothing to print", it is a non-host target's dependency, not a phantom.

2. **Sibling dependencies mask each other.** The Tauri plugins measured one at a time come out at 1 to 3 exclusive crates each, because `tauri` itself is separately a direct dependency and its subtree counts as shared. Measured as one set, `tauri` plus its five plugins cost 31 crates on Windows and 42 on Linux. The tray backends show the same effect in reverse: `tray-icon`, `ksni`, `zbus`, and the `objc2` family measured together cost 0 on Windows and 2 on Linux, because Tauri already carries nearly all of it. Always measure a cohesive stack as one target set.

3. **`workspace-hack` is a normal edge on every member.** cargo-hakari unifies third-party feature sets by making every member depend on one crate that names every unified dependency, so every member's normal closure includes every other member's third-party dependencies. At `92620c7e`, `reqwest 0.13.4` reached the gateway binary through exactly one path, `workspace-hack`, with zero gateway call sites on it; it was there because Tauri and `hf-hub` wanted it elsewhere, and a Workshop-only plugin's feature choice (`rustls/ring`) reached the gateway the same way. Any "who pulls X" question must be read through `workspace-hack`, and any per-binary count must be understood as sitting on the hakari floor described below.

4. **Dev and build edges are not shipped.** `hf-hub`, `safetensors`, `sha2`, and `half` look like part of the picker's runtime graph but are build-dependencies of `promptforge-tool-picker` (they fetch and verify the embedding model at build time). `minijinja` and `minijinja-contrib` are dev-only, the Jinja2 oracle for the bundled chat templates. Restrict to `-e normal` before claiming a runtime dependency exists.

Wall-clock build deltas are not recorded here. Claiming one requires a measured before and after under the same profile, features, and platform.

## Current shape

### The lockfile

- 919 packages in `Cargo.lock`, of which 47 are workspace members.
- Windows (`x86_64-pc-windows-msvc`): 635 third-party packages, 582 distinct names, 48 names resolving at two or more versions.
- Linux (`x86_64-unknown-linux-gnu`): 693 third-party packages, 641 distinct names, 42 names at two or more versions. The Linux figures did not move when the `toml 0.8` line left, because `system-deps` (a build-dependency of the GTK stack) still resolves `toml 0.8.2` there, `gtk3-macros` still resolves `toml_edit 0.19` through `proc-macro-crate 1`, and `glib-macros` still resolves `toml_edit 0.20` through `proc-macro-crate 2`, so `winnow 0.5` keeps two pullers on Linux; the line was ours to drop on Windows and only shared on Linux.
- 146 direct third-party normal dependencies across all members; 30 direct third-party dependencies that are dev-only or build-only.
- 102 third-party packages on Windows and 103 on Linux are reachable only through dev or build edges and never ship.

### What each shipped shape carries

Normal-edge closure, host platform Windows, including the package itself and workspace members it depends on.

| Target | Packages | Notes |
|---|---|---|
| `gateway` (default features: `local`, `web-search`, `config-ui`, `stt`) | 456 | The credential boundary process |
| `gateway --no-default-features` | 427 | Headless: no local inference, no SPA, no STT, no Brave |
| `workshop` (Tauri shell) | 717 | Embeds `workshop-server` and the executor |
| `promptforge-api-runtime` | 509 | The one-door library a host depends on |
| `promptforge-api-types` | 375 | The vocabulary crate; see the hakari floor below |
| `workspace-hack` | 372 | The hakari floor under every member that inherits it |
| `shared-vfs` | 1 | Std only, excluded from hakari, enforced by its manifest test |

Every row but `shared-vfs` dropped by eight or nine packages when the `reqwest 0.12` line and `ring` left the tree. The two gateway rows then dropped by seven and the Workshop row by six when the `toml 0.8` line and its `toml_edit 0.19` and `winnow 0.5` tail left; the library rows did not move because neither library used it. The `promptforge-api-types` row is the one to read twice. A types crate with no I/O ships 375 packages because 372 of them are `workspace-hack`; its own contribution is three. That is the hakari trade stated as a number: `-p` builds share compiled artifacts across the workspace, and in exchange no member build is ever smaller than the unified set. The 29 packages between headless and default gateway are what the four default features actually add.

### Exclusive transitive cost, ranked

Cohesive stacks are measured as one set. Windows figure first, Linux second where they differ.

| Stack | Exclusive crates | What it does | Where |
|---|---|---|---|
| `turso` | 59 | Workspace files: three plain tables in an embedded SQLite-compatible database | `workshop-workspace` |
| `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers` | 50 | Runs the local sentence-embedding model that fills fuzzy tool slots | `promptforge-tool-picker` |
| `tauri` and five plugins | 31 / 42 | The desktop shell | `workshop` |
| `readabilityrs`, `htmd` | 17 | Article extraction and HTML-to-Markdown for web fetch | `promptforge-webfetch` |
| `axum`, `tower`, `tokio-tungstenite` | 10 | HTTP and WebSocket serving | gateway, workshop-server |
| `rust-embed` | 5 | Embeds the two SPAs in release binaries | `gateway-config-ui`, `workshop-server` |
| `sysinfo`, `nvml-wrapper` | 5 / 4 | CPU, RAM, disk, and GPU readings for `GET /admin/system` | `gateway` |
| `mlua` | 4 | The vendored Lua 5.5 VM | `promptforge-lua`, runtime |
| `nvml-wrapper` alone | 3 | GPU name and VRAM, NVML loaded at runtime | `gateway` |
| `serde_yaml_ng` | 2 | Frontmatter parsing | `promptforge-parser` |
| `pulldown-cmark` | 2 | Markdown parsing | `promptforge-parser` |
| `ctrlc` | 2 / 1 | Console interruption in the build orchestrator | `build-workshop` |

Everything not listed costs one crate or zero exclusively: it is either a leaf or entirely shared. `tray-icon`, `objc2` and its framework crates, `ksni`, and `zbus` cost 0 on Windows and 2 on Linux as a set because Tauri already carries the platform stacks.

## What was replaced, and why

Each of these traded a dependency, a build-time requirement, or a whole crate for something the workspace owns. The rule applied throughout matches the workspace principle: reuse an existing facility, then make the smallest improvement to one, then add new machinery only for a material benefit beyond tidiness.

| Removed | Replacement | Evidence and record |
|---|---|---|
| `whisper-rs`, `whisper-rs-sys` (compiled whisper.cpp with cmake inside every `cargo build`) | `gateway-whisper-ffi`: about 300 lines of `libloading` bindings over the pinned whisper.cpp C API, loading a prebuilt `libwhisper` downloaded at runtime | The C API surface was enumerated function by function before the swap (eight calls). Removed cmake, a C++ toolchain, and the CUDA toolkit from `cargo build`, the five-crate `cuda` feature chain, and the `MACOSX_DEPLOYMENT_TARGET` workaround in CI. Design record: `vibe/2026-09/2026-09-02-3-whisper-shared-lib.md` |
| `promptforge-gateway-build` (build-time native compilation) | Prebuilt artifact downloads | Deleted in `ef82879f`; the crate rename plan notes there was nothing left to rename |
| `desktop-shell` (in-house window and webview shell) | Tauri v2 | Migration record: `vibe/2026-08/2026-08-31-6-tauri-migration-for-workshop.md` |
| `promptforge-agent` and the standalone `.lua` program path | The unified Markdown prompt runtime | Removed under the one-door plan, `vibe/2026-09-12-5-one-door-promptforge-api.md`; the path was dead once sections could call Lua directly |
| `mcp-server`, the dev runner, the tape | Nothing; deleted products | Generalized out of the user guide rather than corrected, `vibe/2026-09/2026-09-02-6-product-user-guides.md` |
| `sysinfo` as a prewarm gate | No gate; product design assumes the machine holds the transcription model | `vibe/2026-08/2026-08-29-4-progress-architecture-rollout.md`. `sysinfo` later entered the tree for a different purpose, `GET /admin/system`, with `default-features = false` and only the `system` and `disk` probes. The two decisions are both correct and should not be confused: the first refused a dependency to gate product behavior, the second accepted one to report telemetry |
| `reqwest 0.12` with `rustls-tls` (the `ring` backend and the bundled `webpki-roots` list), beside the `reqwest 0.13` Tauri and `hf-hub` required | `reqwest 0.13` with `rustls` (the `aws-lc-rs` backend, OS trust store through `rustls-platform-verifier`) as the single workspace pin; `tauri-plugin-updater` with `default-features = false` so its `rustls-tls` default stops pinning `rustls/ring` | Before, host Windows: `cargo tree -p gateway -e normal -i rustls@0.23.45 -f '{p} [{f}]' --depth 0` printed `rustls v0.23.45 [aws-lc-rs,aws_lc_rs,ring,std,tls12]`. After: `rustls v0.23.45 [aws-lc-rs,aws_lc_rs,std,tls12]`, and `cargo tree -p gateway -e normal -i ring` prints nothing, likewise for `workshop` and `promptforge-api-runtime` and for `--workspace --target all`. `ring`, `webpki-roots`, and the second `reqwest` left every shipped closure; `ring` remains in `Cargo.lock` only as an unselected optional dependency of `rustls-webpki` and `quinn-proto`. Plan: `vibe/2026-09-18-1-single-rustls-backend.md` |
| `toml 0.8` as the workspace pin, beside the `toml 1.1` that `tauri-utils` resolves | `toml 1` as the workspace pin; no code change, the API in use (`Value`, `Table`, `Spanned`, `from_str`, `to_string_pretty`) is unchanged and every `gateway-config` round-trip and fixture test passed as written | `cargo tree --workspace -e normal --target all -i toml@0.8` prints nothing, so the 0.8 line is off every shipping platform (it survives only as a Linux build edge under `system-deps`, which `-e normal` excludes). With `--target x86_64-pc-windows-msvc` or `--target aarch64-apple-darwin`, `-i toml_edit@0.19` and `-i winnow@0.5` also print nothing; the gateway closure on Windows dropped from 463 to 456. With `--target all` both still resolve, entirely inside Tauri's Linux GTK stack: `toml_edit 0.19.15` via `proc-macro-crate 1.3.1` <- `gtk3-macros 0.18.2` <- `gtk 0.18.2`, and `winnow 0.5.40` via that same `toml_edit 0.19.15` and a second puller, `toml_edit 0.20.2` <- `proc-macro-crate 2.0.2` <- `glib-macros 0.18.5` <- `glib 0.18.5`. Neither `proc-macro-crate` pin is ours; both leave when Tauri's GTK crates move to `proc-macro-crate 3`. Same plan, second commit |

Four of these deserve their reasoning kept.

### Whisper moved out of the build because the toolchain was the cost, not the crate

`whisper-rs` was five crates. What it cost was cmake, a C++ compiler, and on the CUDA path a ten-minute toolkit install in every Windows Workshop nightly, plus a macOS deployment-target hack that never worked in CI. The replacement is a runtime-loaded shared library and one small FFI crate whose `unsafe` is confined to documented blocks at each symbol call, with `undocumented_unsafe_blocks` and `missing_safety_doc` at deny. This is the workspace principle "runtime and serve paths never compile native dependencies" applied to its largest offender, and it is the model for any future native dependency: prebuild, download, load, never compile in the consumer's `cargo build`.

### `serde_yaml_ng` was chosen over `serde_yaml` before the question came up

`serde_yaml` is archived upstream. The parser depends on the maintained fork, at 2 exclusive crates. Nothing to do here; recorded so nobody proposes the swap that already happened.

### `minijinja` is an oracle, not a dependency

The chat-template tests compare the bundled Jinja2 templates against `minijinja` with `pycompat` as a compatibility oracle. It is dev-only and never ships. The runtime does its own substitution. Keep it exactly where it is.

### One rustls backend, enforced by feature resolution rather than by a boot-time install

The workspace pinned `reqwest 0.12`, whose `rustls-tls` feature selects `ring`; Tauri and `hf-hub` pulled `reqwest 0.13`, whose `rustls` feature selects `aws-lc-rs`; `workspace-hack` unified both into every member, so the gateway, the one process holding vendor credentials, compiled and linked two C and assembly cryptography libraries with two advisory streams, and rustls could not pick a `CryptoProvider` on its own. Moving the pin to 0.13 removed the `ring` path that reqwest itself opened, but the tree still selected `ring` after the pin: `tauri-plugin-updater 2.11.0`'s default `rustls-tls` feature depends on `rustls` with `features = ["ring"]` and installs `ring` as the process-global provider when none is set, and hakari carried that feature into every member. Turning the plugin's default features off (keeping `system-proxy` and `zip`) closed the last path; the plugin's own reqwest calls still get TLS because cargo unifies reqwest's features per binary and the workspace pin enables `rustls`.

The alternatives were rejected on the workspace's own rules. Keeping 0.12 with `rustls-tls-no-provider` and installing a provider at boot would put process-global state in a library a host embeds. Excluding the Workshop-only pullers from hakari traversal would fix the gateway by configuration that must be maintained and leave two backends in the Workshop binary. The adopted shape needs no startup test asserting the installed provider: reqwest 0.13's `rustls` feature hands `aws-lc-rs` to the `ClientConfig` builder explicitly, so no provider is installed process-wide and none can be swapped underneath. The remaining invariant, `ring` absent from the gateway's normal closure, is a link-time fact and is checked by one CI step in the `supply-chain` job. Root trust moved from the bundled Mozilla list to the OS store, which is what the Tauri updater and `hf-hub` clients in the same processes already did.

## What deliberately stays

### Correctness-critical dependencies are never reimplementation candidates

Whatever their exclusive cost, these are kept because owning them would mean owning a correctness or security surface the workspace has no business owning:

- `mlua` with vendored Lua 5.5: the sandbox every prompt runs in. Four exclusive crates.
- `rustls`, `rustls-webpki`, `rustls-platform-verifier`, `aws-lc-rs`: TLS for every outbound call from the gateway, with root trust from the OS store.
- `sha2`, `hmac`, `subtle`: the gateway bearer key and its constant-time comparison.
- `turso`: it backs user-visible workspace files, so it is a product surface, not a utility. Exact-pinned at `=0.7.2` because it is pre-1.0 and upgrades must be reviewed changes rather than resolver drift.
- `candle-*` and `tokenizers`: numerical parity with the embedding model the picker was calibrated against. A reimplementation that differed in the last bit would silently change which tool a fuzzy slot fills.
- `tauri`: the desktop shell. Pinned to the minor because `with_webview` exposes platform webview types that shift in minors.
- `reqwest`: the HTTP client on both sides of the gateway wall.

### Version and feature choices already recorded in the manifest

The root `Cargo.toml` comments carry the per-dependency reasoning and this document does not restate it. The ones that exist because of measurement are: `tokio-tungstenite` at 0.29 to unify with the copy axum's `ws` feature pulls; `tokenizers` at 0.22 with `default-features = false` to unify with candle and drop the progress bar and C++ trainer; `tar` and `zip` with defaults off; `sysinfo` reduced to two probes; `ksni` without its tokio feature to avoid the documented runtime-coupling panic class; the `objc2` framework crates feature-gated per class; `turso` with `mimalloc` off so a library cannot install a process-global allocator from a serve path.

### Duplicate versions, and which are ours

Forty-eight names resolve at two or more versions on Windows. Nearly all are transitive and not ours to fix:

- `zip 4` and `8`: `tauri-plugin-updater` pins 4; the workspace uses 8 for release archives.
- `base64 0.13`: `spm_precompiled` under `tokenizers`. `base64 0.21`: `swift-rs` under `tauri-build`, macOS only.
- `rand 0.8`, `0.9`, `0.10`: 0.9 is ours; 0.8 and 0.10 arrive through candle and Tauri.
- `windows-sys` at four versions: 0.61 is ours; `jni` pins 0.45, the rest are Tauri's. The 0.52 line left with `ring`.
- `syn 1`: only `glib-macros` on Linux, under Tauri's GTK stack.
- `toml 0.9` and `1.1`: 1.1 is the workspace pin, shared with `tauri-utils`; 0.9 is `cargo_toml` under `tauri-build`, a build edge that never ships. The 0.8 line that was ours left with the pin move.
- `thiserror 1` and `2`, `hashbrown` at three versions, `indexmap 1`: transitive, chase by upgrading upstreams.

Chase transitive duplicates only by upgrading the upstream that pins them. Do not add a second workspace pin to paper over one.

## Open findings

These are the results of the first measurement that call for a decision. None is applied by this document. Finding 1, the two rustls crypto backends, is resolved and recorded under "What was replaced, and why"; the numbering below is kept stable because other documents cite it.

### Finding 2: `workspace-hack` sets a 372-package floor under every member

`promptforge-api-types` ships 375 packages of which three are its own. Every member that inherits `workspace-hack` builds the unified set, which is the intended trade for shared build artifacts across `-p` invocations. It is recorded here because it changes how every other number in this document reads, and because it was the mechanism behind finding 1: a Workshop-only plugin's feature choice reached the gateway through it. The candidate adjustment is hakari `[traversal-excludes]` for dependencies only one product needs (the Tauri family, `hf-hub`), which would shrink the gateway's floor at the cost of some artifact sharing. Measuring the trade requires editing `.config/hakari.toml` and rebuilding, so it is not measured here. Confidence: medium; the direction is clear, the magnitude is not.

### Finding 3: `turso` is 59 crates for three tables

The largest single exclusive cost in the tree backs a schema the manifest comment describes as "three plain tables". It stays for now because it is a product surface with an exact pin and a stated upgrade discipline. The recorded alternative is `rusqlite` with `bundled`, on the order of five crates, if the sync and replication capabilities that justify `turso` over SQLite are never used. This is an option to keep visible, not a proposal.

### Smaller candidates

- `readabilityrs` and `htmd` at 17 crates for article extraction are fine on count, but both are 0.x. Worth watching rather than acting on.
- `nvml-wrapper` at 3 crates loads NVML dynamically so machines without an NVIDIA driver degrade to an absent field. Keep; the alternative is raw FFI.

## Success bar

- A removal or replacement proposal names the exclusive crate count it eliminates and the call sites it must reproduce, both recomputed from the current `Cargo.lock` with `--filter-platform` on every shipping platform, measuring cohesive stacks as one set and distinguishing normal edges from dev and build edges.
- A reimplementation lands with tests covering the behavior the dependency provided and a comment at the implementation naming the crate it replaced and why. Where the replaced crate produced bytes, the tests carry a fixed vector captured from it.
- Anything touching a correctness-critical dependency listed above is reviewed as a security change, not a cleanup.
- Build-time or artifact-size claims cite measurements taken under the same profile, features, and platform.
- The rustls invariant is checked in CI (`supply-chain` job): `cargo tree -p gateway -e normal -i ring` prints nothing, and the step fails if `cargo tree` itself fails. No provider-install test exists because no provider is installed process-wide; reqwest passes `aws-lc-rs` to rustls explicitly.
- This document is refreshed when a dependency in the ranked table is added, removed, or crosses a major version, with the commit and date in the header updated.

## Reproducing the measurements

```
cargo metadata --format-version 1 --locked --all-features --filter-platform x86_64-pc-windows-msvc
cargo tree -p gateway -e normal --prefix none | sort -u | wc -l
cargo tree -p gateway -e normal --no-default-features --prefix none | sort -u | wc -l
cargo tree -p workshop -e normal --prefix none | sort -u | wc -l
cargo tree -p promptforge-api-runtime -e normal --prefix none | sort -u | wc -l
cargo tree -p workspace-hack -e normal --prefix none | sort -u | wc -l
cargo tree -p gateway -e normal -i rustls@0.23.45 -f '{p} [{f}]' --depth 0
cargo tree -p gateway -e normal -i reqwest
cargo tree -p gateway -e normal -i ring
cargo tree --workspace -e normal --target all -i <crate>
```

The lockfile-shape numbers (third-party packages, distinct names, names at two or more versions, dev-or-build-only packages) come from the `cargo metadata` resolve graph: exclude workspace members, group by name, and take the normal-edge closure from every member for the dev-or-build-only count.

The exclusive-cost ranking is an eighty-line walk over the `cargo metadata` resolve graph: normal-edge closure of the target set minus the normal-edge closure of every other direct third-party dependency, minus workspace members. It is deliberately not checked into the repository. Repository policy binds structural enforcement to explicit approval, and a ranking is an input to a decision, not a gate. If it is ever wanted as a `build-xtask` subcommand, that is the approval to seek.

*2026-09-18 12:55 - claude-fable-5.1*
