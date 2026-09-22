---
name: Fix CI failures and live-state scatter
overview: Fix the three CI failures from the family reorg (stale crate names in ci.yml, workspace-hack in zero-dep shared-vfs, stale STT fixture paths in UI tests) plus the Windows failure-masking step, then a separate commit adding focused AppState accessors so route handlers stop naming the live-state lock directly.
todos:
  - id: ci-package-name
    content: "ci.yml: rename shared-gateway-discovery to gateway-api-discovery (both jobs), add shell: bash to both ownership-race steps"
    status: in_progress
  - id: ci-shared-vfs
    content: Remove workspace-hack from shared-vfs, exclude it in hakari.toml, verify hakari
    status: pending
  - id: ci-ui-fixtures
    content: Fix gateway-stt fixture paths in four workshop-server/ui test files
    status: pending
  - id: ci-verify
    content: Part 1 verification battery, commit CI repair
    status: pending
  - id: snapshot-core
    content: Add focused AppState accessors (routing, config, profile_name, model_allowlist, is_loading, local_children) in lib.rs
    status: pending
  - id: snapshot-sweep
    content: Convert single-field read sites to accessors; keep scoped guards for atomic multi-field reads and writers
    status: pending
  - id: snapshot-verify
    content: Part 2 verification battery, commit
    status: pending
isProject: false
---

# CI Repair + Live-State Snapshot

## Part 1: CI repair (one commit)

All three failures trace to the family reorg (`32e4cf05`) and feature unification (`dc76380d`), not to the decomposition commits. Evidence: [the failing run](https://github.com/cppalliance/promptforge/actions/runs/35255351895) job logs.

### 1a. Stale package name in ci.yml
- [ci.yml:222](c:\Users\Vinnie\cursor\promptforge\.github\workflows\ci.yml) and [ci.yml:288](c:\Users\Vinnie\cursor\promptforge\.github\workflows\ci.yml): `-p shared-gateway-discovery` -> `-p gateway-api-discovery` (the reorg renamed the package; the sweep missed the workflow file).
- The Windows `check-workshop` copy of this step passed only because it runs under default pwsh, which does not stop on native command failure - the stale command failed silently and the step's exit code came from the last (passing) command. Add `shell: bash` to both "Test Gateway process ownership races" steps (bash exists on windows-latest) so the masking stops.

### 1b. shared-vfs zero-dependency rule vs workspace-hack
- [crates/shared-vfs/Cargo.toml](c:\Users\Vinnie\cursor\promptforge\crates\shared-vfs\Cargo.toml) gained `workspace-hack.workspace = true` from the hakari commit; the load-bearing manifest test ([lib.rs:51](c:\Users\Vinnie\cursor\promptforge\crates\shared-vfs\src\lib.rs)) pins zero dependencies and the crate comment says never weaken it.
- Fix the manifest, not the test: remove the workspace-hack line from shared-vfs and exclude shared-vfs from hakari management in [.config/hakari.toml](c:\Users\Vinnie\cursor\promptforge\.config\hakari.toml) (`final-excludes`; verify the exact cargo-hakari config key at execution). shared-vfs has no third-party deps, so feature unification loses nothing.
- Check whether any workflow step runs `cargo hakari verify` and confirm it passes with the exclusion.

### 1c. Stale STT fixture paths in UI tests
- Four files in [crates/workshop-server/ui/test/](c:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui\test) - `stt-stream.mjs:13`, `agent-stt.mjs:22`, `realtime-wire-fixtures.mjs:26`, `pcm-worklet.mjs:14` - join `../../..` + `gateway-stt/tests/fixtures/...`. The reorg moved that tree to `crates/gateway/stt/api/tests/fixtures/` (verified on disk). Replace the `"gateway-stt"` segment with `"gateway", "stt", "api"` in each path.join.

### Part 1 verification
- `cargo test -p shared-vfs` (manifest test green)
- `cargo test --locked -p gateway-api-discovery a_process_lifetime_lease_recovers_after_its_owner_is_terminated` (the exact command CI runs)
- `npm test` in `crates/workshop-server/ui`
- `cargo hakari verify` if configured in any workflow
- `rg "shared-gateway-discovery|gateway-stt" .github/ crates/workshop-server/ui` returns zero hits

## Part 2: AppState accessors (separate commit, after Part 1)

The scatter is ~25 production `state.live.read()/write()` sites across 15 files. Two designs were rejected: a guard-holding extractor (holds a read lock for whole requests, blocking config applies during long streams) and a snapshot-struct extractor (a new centralized concept with staleness semantics to teach; `LocalRuntime` is not `Clone`, so the snapshot would be partial anyway). The fix is the decentralized idiom the crate already has - `web_search()`, `cache_dir()`, `tray_model_status()` - extended:

- New focused accessors on `AppState` in [lib.rs](c:\Users\Vinnie\cursor\promptforge\crates\gateway\app\src\lib.rs), each a one-line read under the guard: `routing() -> Arc<Routing>`, `config() -> Arc<Config>`, `profile_name() -> Option<String>`, `model_allowlist() -> Option<Vec<String>>`, `is_loading(name) -> bool`, `local_children() -> usize`.
- Convert single-field read sites to the accessors: config_pending.rs, config_apply.rs's read-only spots, and the one-field reads in route handlers.
- Keep one scoped `let live = state.live.read().await;` where a handler reads several fields atomically (admin/status.rs, relay.rs's `resolve_routed_model`, models.rs, speech.rs's `audio_voices`): the guard is the correct tool there, and splitting it into separate accessor calls would let a config apply land mid-read.
- Keep direct guards for: writers (`config_apply`, `boot_load`, `commands`), the auth hot path (`check_auth`, handoff's key read), and non-route machinery (`runner.rs`).
- Extend the crate-docs placement paragraph: handlers read live state through `AppState` accessors; a handler needing several fields atomically takes one scoped read; only commands and writers take the write guard.

### Part 2 verification
- Full battery: `cargo test -p gateway` (counts unchanged), clippy `-D warnings`, fmt, doc `-D warnings`, headless check, nextest `--all-features`, `cargo test -p build-xtask`.

## Commit order
1. Part 1 (CI repair) - unblocks the pipeline.
2. Part 2 (AppState accessors) - lands on a green tree.

Stop condition: two consecutive failures on one step stops the run for a re-plan.