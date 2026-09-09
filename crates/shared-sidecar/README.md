# shared-sidecar

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The shared sidecar discovery seam for PromptForge: the `gateway.json` gateway discovery file the gateway writes after a successful bind, Jupyter-style - port, bearer key, pid, boot epoch, version, start time - plus everything a reader needs to attach to a running gateway instead of launching a second one: validation, stale detection (one stable OS process boot bracketing same-socket health and bearer proofs) with stale-file cleanup, the `gateway.json.lock` launch-race lock with loser-attaches-to-winner semantics, and the raw-`TcpStream` health wait. Synchronous and runtime-agnostic: no tokio, axum, or reqwest, so the gateway's lean builds and the workshop readers share one contract.

## Public surface

- `GatewayDiscoveryFile` - the `gateway.json` document, with `read`, `write_to` (atomic, owner-only: mode `0600` on Unix, best-effort via the user profile's ACL on Windows), and `remove_if_mine` for clean shutdown; debug output redacts bearer and untrusted string metadata.
- `ValidatedConnection` - an unforgeable point-in-time live-connection capability created only after one unchanged OS process boot brackets same-socket health and bearer acceptance checks; external test fixtures cannot choose the accepted image, and debug output redacts bearer and untrusted string metadata.
- `resolve` - stale detection: attach parameters for a live gateway, or stale-file cleanup plus the reason.
- `launch_or_attach` - the launch-race lock: the winner launches, losers attach to the winner.
- `request_shutdown` - post the authenticated shutdown request for a validated local Gateway capability.
- `wait_for_health` - poll `GET /health` until it answers 200 or the timeout elapses.
- `CancellationToken` - a clonable signal that wakes bounded sidecar work and linearizes cleanup or other effects so none begin after cancellation returns.
- `resolve_cancellable` - resolve and validate while allowing cancellation to stop probes and prevent stale-file deletion.
- `launch_or_attach_cancellable` - settle the launch race while allowing cancellation to stop lock waits and prevent a later launch decision.
- `wait_for_health_cancellable` - poll health with cancellation, timed connects, one absolute deadline per attempt, and bounded response framing.
- `ValidationError` and `ValidatedConnection::validate_cancellable` - distinguish cancellation from a stale identity without weakening the validated capability.
- `run_dir` / `default_run_dir` / `gateway_discovery_file_path` / `lock_file_path` - the path layout under `<home>/.promptforge/run`.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
