# harness-sessions

This crate owns the harness's session layer: agent discovery, the run lifecycle and supervisor reducer, and the user-input wait registry with the input performer over it.

- The input broker backs only the script-side `user_input()` function. No `user_input` tool is ever advertised to a model unless a prompt explicitly adds it.
- A dying input wait is an outcome, never silence: every path out of an unresolved wait removes the registry entry and pushes a durable `WaitFrame::Cancelled`. Unresolved waits are retained across socket loss and re-announced on reconnect.
- Wait frames are harness data, not wire shapes. The client that owns a socket renders them into its own protocol; this crate never names a `workshop-*` frame type.
- The supervisor's state transitions are a pure reducer whose matches stay wildcard-free, so a new variant is a compile error.
- Family rules: depends on `promptforge-api-runtime`, `promptforge-api-types`, and container siblings only. Never on a `workshop-*` crate, a private `gateway-*` crate, or a `promptforge-*` crate behind the door. Tests spawn through `harness-runner`'s instrumented wrapper, never `tokio::spawn`.
