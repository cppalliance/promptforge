# promptforge-lua

The PromptForge sandboxed Lua runtime. A section's Lua chunk runs in a fresh,
restricted `mlua` VM: only the `string`, `table`, and `math` standard
libraries plus safe base functions, an instruction-count hook, host tables
for the run-scoped store, model and tool bindings, and the coroutine
yield/resume protocol that lets suspending host calls (`models.infer`,
`execute`, `fanout`) run under the executor's scheduler without blocking a
worker thread.
