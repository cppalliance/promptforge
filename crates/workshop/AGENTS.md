# workshop

This crate owns the desktop shell and its product lifecycle.

- Unsafe is confined to the Windows bridge (`src/bridge.rs`): dense working COM with documented failure modes and the crate's only unsafe code; its module-level `#[expect(unsafe_code)]` is deliberate, and every unsafe block carries a `// SAFETY:` comment on the immediately preceding line. Do not restructure it casually, and never edit it without running its tests. No other module contains unsafe code.
- Discovery, server-spawn, health-wait, window, and webview boot failures surface loudly with their full error chain.
- The running event loop degrades and reports recoverable bridge failures instead of crashing the window.
- Gateway launch is detached from the shell through the shared-sidecar launch contract. The shell never hosts the Gateway in-process.
- The shell does not read Gateway configuration, own the Gateway connection file, or kill the Gateway as part of ordinary shell teardown.
- Quit requests authenticated shutdown only for a sidecar-attached Gateway. A LAN-configured Gateway remains running.
- Build the window capability programmatically for the exact bound port. Do not replace it with a wildcard-port capability file.
