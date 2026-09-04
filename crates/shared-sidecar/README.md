# shared-sidecar

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The shared sidecar discovery seam for PromptForge: the `gateway.json` connection file the gateway writes after a successful bind, Jupyter-style - port, bearer key, pid, boot epoch, version, start time - plus everything a reader needs to attach to a running gateway instead of launching a second one: validation, stale detection (live pid + gateway process image + health answer + accepted key) with stale-file cleanup, the `gateway.json.lock` launch-race lock with loser-attaches-to-winner semantics, and the raw-`TcpStream` health wait. Synchronous and runtime-agnostic: no tokio, axum, or reqwest, so the gateway's lean builds and the workshop readers share one contract.

## Public surface

- `ConnectionFile` - the `gateway.json` document, with `read`, `write_to` (atomic, owner-only: mode `0600` on Unix, best-effort via the user profile's ACL on Windows), and `remove_if_mine` for clean shutdown.
- `resolve` - stale detection: attach parameters for a live gateway, or stale-file cleanup plus the reason.
- `launch_or_attach` - the launch-race lock: the winner launches, losers attach to the winner.
- `wait_for_health` - poll `GET /health` until it answers 200 or the timeout elapses.
- `run_dir` / `default_run_dir` / `connection_file_path` / `lock_file_path` - the path layout under `<home>/.promptforge/run`.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
