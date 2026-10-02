# harness-sessions

This crate owns the Harness's session layer: the `Harness` handle and the bindings the Host pushes through the public API, agent discovery, the session runtime (launch through `prepare_source` and `drive_run`, input, cancel, close, event and delta subscriptions, transcript reads from the run log), the run lifecycle and supervisor reducer, and the user-input wait registry and the session broker, `SessionInputBroker` (in `src/input-tool.rs`), which implements `InputBroker` for the `promptforge/user-input` capability. The dependency rules and the core invariants are in the `## Invariants` block of `src/lib.rs`; this file holds only what that block does not say.

- Unresolved input waits are retained across socket loss and re-announced on reconnect; the wait's lifetime is the run's, not the socket's.
- Wait frames are Harness data, not wire shapes. The Host that owns a socket renders them into its own protocol; this crate never names a `workshop-*` frame type.
