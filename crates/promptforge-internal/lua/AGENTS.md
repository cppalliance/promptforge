# promptforge-lua

- Engine functions that would create a parser-to-Lua dependency cycle stay in this crate rather than `promptforge-parser`.
- `prepare_dispatch` is the single tool-dispatch body used by every executor: synchronous, it applies counts, trust classification, the nonce wrap, and the `ToolResult` report to a tool's answer. Nothing in this crate performs a tool call: the executor issues the call as an effect, the Harness performs it, and `prepare_dispatch` (or `prepare_model_dispatch` under the model-issued rule) applies the rules when the answer lands.
- Engine globals are namespace functions over plain values; handles are frozen, inspectable userdata with no methods. New operations go in the owning namespace with an optional leading handle argument - do not add colon methods. Chainable `messages.new()` builders are the deliberate exception.
