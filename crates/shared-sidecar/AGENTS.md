# shared-sidecar

The single shared seam for gateway connection-file discovery: the `gateway.json` format, its atomic owner-only write and shutdown removal, stale detection with cleanup, the `gateway.json.lock` launch-race lock, and the health probe.

- This crate is the only implementation of connection-file discovery: the gateway writes through it, and readers (workshop-server, the workshop shell) attach through it. Never reimplement the file format, the probes, or the lock in a consumer - all sides call through here so the contract cannot drift.
- Synchronous and runtime-agnostic: serde, serde_json, and thiserror are the only general dependencies. No tokio, axum, reqwest, or async anything - the gateway's lean `--no-default-features` build takes this crate unconditionally. Platform shims (windows-sys on Windows, libc on macOS) are the only exception and are confined to `src/sys/`.
- Unsafe is confined to the `src/sys/` process-image shims, each module carrying `#[expect(unsafe_code)]` with its reason and every block a `// SAFETY:` comment; the crate-level lint is `deny` everywhere else.
- Probes normalize to a literal `127.0.0.1`, never `localhost`, and send the bound address as the `Host` header, matching the gateway's loopback `Host` allowlist.
- Stale-file deletion is the launch-lock holder's privilege; a race loser only ever attaches, never deletes. A platform without a process-image shim fails closed: every file reads as stale, so readers relaunch rather than attach to an unverified process.
