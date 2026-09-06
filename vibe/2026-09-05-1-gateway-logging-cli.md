---
name: gateway-logging-cli
overview: Separate Gateway CLI simplification, diagnostic discovery, and asynchronous bounded logging from the STT redesign. The work creates a small `gateway-logging` crate, removes the `serve` verb safely across every repository-owned caller, and makes failed runs discoverable without config knowledge.
todos:
  - id: logging-cli
    content: Characterize and migrate the Gateway CLI and every owned caller
    status: completed
  - id: logging-crate
    content: Extract gateway-logging with bounded prioritized worker and shutdown ownership
    status: completed
  - id: logging-diagnostics
    content: Add diagnostics, retention, fatal-chain capture, privacy, and pressure behavior
    status: pending
  - id: logging-verify
    content: Update rules/docs and complete full verification
    status: pending
  - id: baseline-ratchet
    content: Repair the pre-existing Workshop module ratchets
    status: completed
  - id: plan-activation
    content: Commit this plan and activate it in vibe/ACTIVE
    status: completed
isProject: false
---

# Gateway Logging and CLI

## Design rationale

*2026-09-05 - distilled from the producing session*

The operator chose a separate logging crate because queue ownership, sink lifecycle, rotation, diagnostics, pressure behavior, and tests form an independently testable component. The operator also chose to remove the `serve` verb, use `--config PATH` for an explicit config, and make `diagnostics` emit JSON without an extra format flag.

The final pressure policy is selective rather than fully lossless: on a full bounded queue, evict oldest Debug, then Trace, then Info; Warn and Error are never evicted; block only when the incoming level has no eligible lower-or-equal-priority record to evict. This supersedes the earlier zero-loss-for-all-levels idea. The queue remains bounded so a stalled sink cannot consume unbounded memory.

Rejected alternatives: an unbounded intrusive list, because slow output becomes memory growth; synchronous writes on producer threads, because console or disk stalls can enter realtime paths; `shared-logging`, because only Gateway uses the component; a hidden `serve` compatibility alias, because it leaves a temporary CLI branch; configurable log paths, because config failures need a destination before config is usable; and separate human/JSON diagnostics modes, because one formatted JSON contract serves both.

This is a Full-sized rulebook task because it changes a public CLI, installer and service callers, process startup/shutdown, and logging infrastructure. The run is deliberately lightened to four commits, one Coder and one Review-and-Fix pass per commit, and Verify only at rulebook-required component boundaries, every third commit, review-dirty steps, and final completion.

## Outcome
- `promptforge-gateway` serves by default.
- `promptforge-gateway --config PATH` serves an explicit config.
- `promptforge-gateway diagnostics` emits formatted JSON without serving or rotating logs.
- Remove the `serve` verb, positional config path, and compatibility alias in one atomic migration.
- Extract logging from `gateway/src/main.rs` into `crates/gateway-logging` without moving global subscriber initialization out of the binary.
- Keep log memory bounded and preserve Warn/Error records through priority-aware eviction and blocking.

## Public API and dependency boundary
- Add flat workspace crate `gateway-logging`; `gateway` is its only workspace consumer.
- `gateway-logging` depends only on the standard library, `tracing`, and `tracing-subscriber`. It never reads home, environment, Gateway config, sidecar state, or STT types.
- Export at most `LogConfig`, `LogRuntime`, `LogWriter`, and opaque `LogError`.
- `LogRuntime::start(LogConfig)` creates queues, sinks, and one worker thread. `LogRuntime::writer()` returns cloneable `LogWriter`. `LogRuntime::shutdown(self)` closes admission, drains, flushes, and joins.
- `LogWriter` implements `MakeWriter::make_writer_for` and derives priority only from tracing metadata. The returned `LogWriter` buffers all `Write` calls for one formatted event and enqueues its owned `Box<str>` on drop, so partial formatter writes never become partial queue records.
- Queue nodes, sinks, rotation, mutexes, condition variables, and worker handles stay private.
- Gateway `main.rs` composes and globally installs the subscriber, holds `LogRuntime`, and shuts it down last.
- The crate sets `unsafe_code = "forbid"` in its manifest lint table. Public fields stay private; `LogError` preserves private sources and exposes classification methods. Every public item has rustdoc, error documentation, and compiled examples.

## CLI contract
- Root invocation serves with existing discovery: `promptforge-gateway`.
- Explicit config uses `promptforge-gateway --config PATH`; it wins over `PROMPTFORGE_GATEWAY_CONFIG`.
- `promptforge-gateway diagnostics` is the only subcommand and always emits JSON.
- `--help`, `--version`, `diagnostics`, and second-instance handoff never initialize or rotate file logs.
- Update all owned callers atomically: `gateway/src/main.rs`, `gateway/src/boot.rs`, `gateway/src/tray/logic.rs`, `gateway/packaging/gateway.service`, `gateway/tests/it/boot.rs`, `workshop/src/gateway.rs`, `workshop/installer.nsi`, `.github/workflows/gateway-release-test.yml`, Gateway README, and install guide. Preserve `--login`, `--browser`, `--print-url`, `--no-tray`, and `--profile` behavior.

## Logging path and lifecycle
- Gateway alone derives the state directory from `shared_sidecar::default_run_dir().parent()` and passes it to `LogConfig`.
- Logs remain under `<home>/.promptforge/logs`; this is not configurable because config discovery and parsing failures need a destination.
- Startup order: parse CLI; handle help/version/diagnostics; detect an already-running Gateway; resolve state directory; start logging; resolve or generate config; log version/config/profile; create runtime; bind; write `gateway.json`; enqueue boot loading; enter event loop.
- Shutdown order: stop HTTP and tray work; stop commands, progress renderers, model runtimes, and native callbacks; log the terminal outcome; shut the logger down last.
- Fatal returned errors are logged once with the complete source chain, then the queue drains before process exit. Raw stderr is only the fallback when logger initialization fails.
- Retain `gateway.log` plus `gateway.log.1` through `gateway.log.5`. Rotate only when this process will serve, never during diagnostics or second-instance handoff.

## Queue policy
- Constants: total capacity 8192 records, drain batch 256 records, retained runs 5.
- Private types: `LogPriority { Error, Warn, Info, Trace, Debug }`, `LogRecord { sequence, priority, line: Box<str> }`, `LogQueue`, and `LogWorker`.
- Keep one deque per priority under one mutex and one fixed total capacity. Formatting and allocation happen before locking. The worker swaps a bounded batch to local storage and performs all writes outside the mutex.
- On full queue, evict oldest Debug, then oldest Trace, then oldest Info. Warn and Error are never evicted.
- Prevent inversion: Debug may evict only Debug; Trace may evict Debug or Trace; Info may evict Debug, Trace, or Info; Warn/Error may evict Debug, Trace, or Info. If no eligible record exists, block on a condition variable until space opens.
- Count evictions by level and emit one synthetic summary after pressure clears. Select the smallest global sequence among lane heads so retained output remains chronological.
- File-sink failure falls back to synchronous stderr. If every sink blocks and no eligible record exists, producer blocking is intentional.
- No log record may contain credentials, cookies, authorization headers, environment values, request bodies, audio, transcript text, prompts, or full local model paths.

## Diagnostics contract
`promptforge-gateway diagnostics` returns formatted JSON with:
```json
{
  "state_dir": "...",
  "config": { "path": "...", "exists": true },
  "logs": {
    "current": { "path": ".../gateway.log", "exists": true },
    "retained": [
      { "path": ".../gateway.log.1", "exists": true }
    ]
  },
  "connection_file": { "path": ".../run/gateway.json", "exists": false },
  "running": false,
  "version": "0.2.0"
}
```
- It performs no logging initialization, rotation, config parsing, or mutation.
- It returns no bearer key, environment value, config content, or log content.
- Generated `gateway.toml` adds `# Diagnostics: promptforge-gateway diagnostics` as a comment, not a config field.

## Numbered commits
1. Characterize current CLI, handoff, and launcher behavior in tests, then remove `serve` and positional config, add root serving with `--config PATH`, and update every repository-owned caller atomically. Focused gate: `cargo test -p gateway -p workshop`, followed by the Gateway release-test command fixture.
2. Add `gateway-logging`, move rotation/sink/worker ownership out of `gateway/src/main.rs`, install the bounded priority queue, preserve the default filter, and wire shutdown-last behavior. Focused gate: `cargo test -p gateway-logging -p gateway`.
3. Add five-run retention, JSON `diagnostics`, generated-config hint, fatal-chain capture, handoff-no-rotation, sink fallback, privacy, saturation, shutdown, and release-mode latency tests. Focused gate: `cargo test -p gateway-logging -p gateway`, followed by ignored release test `production_logging_stays_within_latency_budget`; normal sink throughput must add less than 2% or 1 ms, whichever is larger, to p95 enqueue-to-write latency.
4. Update AGENTS and final documentation, enforce the dependency boundary, and run full verification: formatting, all-target/all-feature Clippy with warnings denied, `cargo test -p gateway-logging -p gateway -p workshop`, Gateway featureless check, rustdoc, `cargo deny check`, packaged Gateway/Workshop builds, and a child-process failure followed by `diagnostics` log discovery.

## Execution rules
- Repository: `C:\Users\Vinnie\cursor\promptforge`.
- Plan: `C:\Users\Vinnie\.cursor\plans\gateway-logging-cli_d7a036c4.plan.md`.
- Rulebooks: `C:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md` and `C:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md`.
- Governing rules: root `AGENTS.md`, `crates/gateway/AGENTS.md`, `crates/workshop/AGENTS.md`, `crates/shared-sidecar/AGENTS.md`, and new `crates/gateway-logging/AGENTS.md` after commit 2.
- Scratch: `C:\Users\Vinnie\cursor\cabinet\_scratch\vibe-gateway-logging-cli\vibe-ledger.md` and `vibe-review.md`.
- Prerequisite: the separately accepted Workshop module-ratchet baseline task is green, the full existing suite passes, and the worktree is clean. Stop rather than stash or absorb unrelated changes.
- Before commit 1, copy this plan to the next dated `vibe/<date>-<N>-gateway-logging-cli.md`, write its basename to `vibe/ACTIVE`, generate the plan commit message through the vibe rulebook, and commit both files.
- Each numbered item is one commit with code and tests. Use asynchronous Coder, Message, and Review-and-Fix subagents. Open findings block advancement.
- Light Verify schedule: focused tests only after commit 1 unless review changes code; fresh Verify after commit 2 as a component boundary, commit 3 as every third step and component boundary, and commit 4 with the full suite.
- Before every commit, run stable formatting checks and all-target/all-feature Clippy with warnings denied plus the focused gate. Public items receive rustdoc and tests in the same change.
- Two consecutive implementation failures or three failed verification rounds stop execution for re-planning. The tool never pushes.
