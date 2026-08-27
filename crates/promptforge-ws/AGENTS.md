# Desktop Shell Rules

These rules bind `crates/promptforge-ws`. The repo-root AGENTS.md applies on top.

## Two-zone error policy

Zone one is config discovery plus window and server construction: fail loudly and immediately. Zone two is the running event loop: never panic; degrade and report rather than crash the window.

## file_drop.rs is a guarded module

`src/file_drop.rs` is dense working COM with documented failure modes and the workspace's only unsafe code; its module-level lint allowances are deliberate. Do not restructure it casually, and never edit it without running its tests.
