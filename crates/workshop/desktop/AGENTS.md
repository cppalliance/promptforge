# workshop

- Unsafe is confined to the Windows bridge (`src/bridge.rs`): dense working COM with documented failure modes and the crate's only unsafe code; its module-level `#[expect(unsafe_code)]` is deliberate. No other module contains unsafe code.
- The desktop app does not read Gateway configuration, own the Gateway discovery file, or kill the Gateway as part of ordinary teardown.
