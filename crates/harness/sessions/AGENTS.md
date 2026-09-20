# harness-sessions

This crate owns the harness's session layer: the `Harness` handle and the bindings a client pushes across the door (gateway, chat catalog, host snapshot), agent discovery, the session runtime (launch through `prepare_source` and `drive_run`, input, cancel, close, event and delta subscriptions, transcript reads from the run log), the run lifecycle and supervisor reducer, and the user-input wait registry with the input performer over it. The dependency rules and the core invariants are in the `## Invariants` block of `src/lib.rs`; this file holds only what that block does not say.

- The capability registry and the model client are rebuilt when the gateway generation changes; a binding update never patches a live registry in place.
- Unresolved input waits are retained across socket loss and re-announced on reconnect; the wait's lifetime is the run's, not the socket's.
- Wait frames are harness data, not wire shapes. The client that owns a socket renders them into its own protocol; this crate never names a `workshop-*` frame type.
