# promptforge-workshop

These rules bind `crates/promptforge-workshop`. The repo-root AGENTS.md
applies on top.

## Scope

This crate is the desktop app: the Tauri window pointed at the in-process
gateway's workshop UI, the same-origin navigation policy, the platform
bridges, and the lifecycle orchestration (configuration discovery, gateway
start, the health wait, shutdown). The UI itself is served by the gateway
and lives in `promptforge-workshop-server/ui`; this crate never bundles
frontend assets (`frontendDist` stays unset, `app.windows` stays empty -
the window is created programmatically once the health probe answers).

## Unsafe is confined to the Windows bridge

`src/bridge.rs` is dense working COM with documented failure modes and the
crate's only unsafe code; its module-level `#[expect(unsafe_code)]` is
deliberate, and every unsafe block carries a `// SAFETY:` comment on the
immediately preceding line. Do not restructure it casually, and never edit
it without running its tests. No other module in this crate contains
unsafe code.

## Event-loop error policy

Boot failures (discovery, gateway spawn, the health wait) print their full
error chain and exit with a failure code. Window and webview construction
fails loudly through the setup hook. The running event loop never panics:
degrade and report rather than crash the window - a failed bridge attach
loses Explorer drops and the mic grant, not the app.
