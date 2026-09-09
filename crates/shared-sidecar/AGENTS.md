# shared-sidecar

This crate owns the shared Gateway discovery-file and sidecar lifecycle seam.

- This is the only implementation of gateway-discovery-file discovery, atomic owner-only publication, shutdown removal, stale resolution, launch locking, health probing, and authenticated shutdown. Consumers do not reimplement those contracts.
- Keep the crate synchronous and runtime-independent so a lean Gateway build can always use it.
- Unsafe is confined to the `src/sys/` process-image shims, each module carrying `#[expect(unsafe_code)]` with its reason and every block a `// SAFETY:` comment; the crate-level lint is `deny` everywhere else.
- Probes normalize to a literal `127.0.0.1`, never `localhost`, and send the bound address as the `Host` header, matching the gateway's loopback `Host` allowlist.
- Stale-file deletion is the launch-lock holder's privilege; a race loser only ever attaches, never deletes. A platform without a process-image shim fails closed: every file reads as stale, so readers relaunch rather than attach to an unverified process.
