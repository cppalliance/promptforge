# promptforge-lua

The PromptForge sandboxed Lua runtime. A section's Lua chunk runs in a fresh,
restricted `mlua` VM: only the `string`, `table`, and `math` standard
libraries plus safe base functions, an instruction-count hook, Engine globals
for the run-scoped store, model and tool bindings, and the coroutine
yield/resume protocol that lets suspending Engine calls (`models.infer`,
`call`, `fanout`, `tools.call`) run under the executor's scheduler without
blocking a worker thread. One guard metatable on `_G` serves the frozen
`argv` and the lazy, read-only `prose`; an author's own `_G` metatable
composes behind it through the sandbox's `setmetatable` and
`getmetatable`, which can neither reveal nor replace the guard. Every
name `_G` holds after section setup, plus the Lua keywords, is on one
reserved-name list: no frontmatter tool alias, model role label, or
capability prelude global may take one.

Beside the core globals, the VM holds:

- `store`, the run's virtual files, backed by the `promptforge-vfs` store
  view the executor derives for the chain; each operation is a direct call
  while the shared library loads and a yield shim inside a block.
- `messages`, the conversation builders and the chainable `messages.new()`.
- `tasks`, the scheduler-mode shims for spawning, waiting on, checking,
  noting, and cancelling tasks and reading their event history.
- Capability preludes: the Lua source an activated capability contributes,
  run once per VM in an environment of its own before the shared library
  replays, with its globals checked against the reserved list and raw-set
  into `_G`. The `input` table that `promptforge/user-input` defines is
  one: `input.ask()` is an ordinary tool call to that capability's ask
  tool.
