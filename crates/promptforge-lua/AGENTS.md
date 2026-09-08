# promptforge-lua

This crate owns the sandboxed Lua runtime, its host surface, and coroutine protocol vocabulary.

- Host functions that would create a parser-to-Lua dependency cycle stay in this crate rather than `promptforge-parser`.
- Executors drive this crate. It never imports or composes an executor.
- `dispatch_tool` is the single tool-dispatch body used by every executor.
- Hidden cross-crate seams for executors are not host API and must not gain documented status without a design change. `LuaProgram` remains genuine API.
