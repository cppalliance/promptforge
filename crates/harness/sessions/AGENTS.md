# harness-sessions

This crate owns the harness's session layer: the `Harness` handle and the bindings a client pushes across the door (gateway, chat catalog, host snapshot), agent discovery, the session runtime (launch through `prepare_source` and `drive_run`, input, cancel, close, event and delta subscriptions, transcript reads from the run log), the run lifecycle and supervisor reducer, and the user-input wait registry with the input performer over it.

- Every binding a run reads arrives as data through `harness-api`; this crate never resolves a gateway or reads a client's state. It is the one place a capability provider crate (`harness-web`) is named, at registration; the registry and model client are rebuilt when the gateway generation changes.
- A session's transcript is the run log. The live event broadcast and `Session::transcript` agree index for index, and the reply-id stamp is one rule (`session::reply_stamp`) applied to both.

- The input broker backs only the script-side `user_input()` function. No `user_input` tool is ever advertised to a model unless a prompt explicitly adds it.
- A dying input wait is an outcome, never silence: every path out of an unresolved wait removes the registry entry and pushes a durable `WaitFrame::Cancelled`. Unresolved waits are retained across socket loss and re-announced on reconnect.
- Wait frames are harness data, not wire shapes. The client that owns a socket renders them into its own protocol; this crate never names a `workshop-*` frame type.
- The supervisor's state transitions are a pure reducer whose matches stay wildcard-free, so a new variant is a compile error.
- Family rules: depends on `promptforge-api-runtime`, `promptforge-api-types`, and container siblings only. Never on a `workshop-*` crate, a private `gateway-*` crate, or a `promptforge-*` crate behind the door. Tests spawn through `harness-runner`'s instrumented wrapper, never `tokio::spawn`.
