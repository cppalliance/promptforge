# Desktop Binary Rules

These rules bind `crates/promptforge-workshop`. The repo-root AGENTS.md applies on top.

## Two-zone error policy

Zone one is config discovery plus gateway construction: fail loudly and immediately. Zone two is the running event loop: never panic; degrade and report rather than crash the window. The event loop lives in `promptforge-desktop-shell`, which owns the zone-two policy for the code it hosts.

## Lifecycle orchestration only

The desktop binary remains lifecycle orchestration: configuration discovery, gateway start, the health wait, shutdown, and feature forwarding. It drives the window through the single `promptforge-desktop-shell::run` entry point and does not reacquire GUI implementation dependencies (tao, wry, or the Windows COM crates). The WebView2 file-drop bridge moved with the shell; its guarded-module rules live in that crate's AGENTS.md.
