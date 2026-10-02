# workshop

- Unsafe is confined to the Windows bridge (`src/bridge.rs`): dense working COM with documented failure modes and the crate's only unsafe code; its module-level `#[expect(unsafe_code)]` is deliberate, and every unsafe block has a `// SAFETY:` comment on the immediately preceding line. No other module contains unsafe code.
- The desktop app does not read Gateway configuration, own the Gateway discovery file, or kill the Gateway as part of ordinary teardown.
