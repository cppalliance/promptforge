# promptforge-lua

This crate owns the sandboxed Lua runtime, its host surface, and coroutine protocol vocabulary.

- Host functions that would create a parser-to-Lua dependency cycle stay in this crate rather than `promptforge-parser`.
- Executors drive this crate. It never imports or composes an executor.
- `prepare_dispatch` is the single tool-dispatch body used by every executor: synchronous, it applies counts, trust classification, the nonce wrap, and the `ToolResult` report to a tool's answer. Nothing in this crate performs a tool call: the executor issues the call as an effect, the host performs it, and `prepare_dispatch` (or `prepare_model_dispatch` under the model-issued rule) applies the rules when the answer lands.
- The executor-facing items are public for `promptforge-engine`, not host API: the facade re-exports only `StoreOp` and `StoreOutcome`, and none of them is `#[doc(hidden)]` (the `build-xtask` ban rejects it). An operation only the engine performs on a host-visible type is a free function in `detail`; a helper only tests call sits behind `test-support`.
- Lua host capabilities are namespace functions over plain values; handles are frozen, inspectable userdata with no methods. New operations go in the owning namespace with an optional leading handle argument - do not add colon methods. Chainable `messages.new()` builders are the deliberate exception.
