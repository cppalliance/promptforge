# promptforge-lua

This crate owns the sandboxed Lua runtime, its host surface, and coroutine protocol vocabulary.

- Host functions that would create a parser-to-Lua dependency cycle stay in this crate rather than `promptforge-parser`.
- Executors drive this crate. It never imports or composes an executor.
- `dispatch_tool` is the single tool-dispatch body used by every executor.
- Hidden cross-crate seams for executors are not host API and must not gain documented status without a design change. `LuaProgram` remains genuine API.
- Lua host capabilities are namespace functions over plain values; handles are frozen, inspectable userdata with no methods. New operations go in the owning namespace with an optional leading handle argument - do not add colon methods. Chainable `messages.new()` builders are the deliberate exception.
