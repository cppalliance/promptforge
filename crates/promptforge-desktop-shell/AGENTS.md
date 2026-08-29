# promptforge-desktop-shell

These rules bind `crates/promptforge-desktop-shell`. The repo-root
AGENTS.md applies on top.

## Scope

This crate owns windowing, the WebView, IPC, and the platform bridges -
nothing else: the tao/wry event loop, window creation, the
custom-title-bar IPC commands, the navigation policy, the microphone
permission grant, file drops, and the program icon. Lifecycle
orchestration (configuration discovery, gateway start, the health wait,
shutdown) stays in the `promptforge-ws` binary, which drives this crate
through the single documented `run` entry point. Never depend on the
gateway or any other PromptForge crate.

## Unsafe is confined to the Windows bridge

`src/file_drop.rs` is dense working COM with documented failure modes and
the workspace's only unsafe code; its module-level lint allowances are
deliberate. Do not restructure it casually, and never edit it without
running its tests. No other module in this crate contains unsafe code.

## Event-loop error policy

Window and webview construction fails loudly, returning the error to the
caller. The running event loop never panics: degrade and report rather
than crash the window.
