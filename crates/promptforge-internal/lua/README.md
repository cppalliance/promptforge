# promptforge-lua

The PromptForge sandboxed Lua runtime. A section's Lua chunk runs in a fresh,
restricted `mlua` VM: only the `string`, `table`, and `math` standard
libraries plus safe base functions, an instruction-count hook, host tables
for the run-scoped store, model and tool bindings, and the coroutine
yield/resume protocol that lets suspending host calls (`models.infer`,
`call`, `fanout`, `tools.call`) run under the executor's scheduler without
blocking a worker thread. One guard metatable on `_G` serves the frozen
`argv` and the lazy, read-only `prose`; an author's own `_G` metatable
composes behind it through the sandbox's `setmetatable` and
`getmetatable`, which can neither reveal nor replace the guard.
