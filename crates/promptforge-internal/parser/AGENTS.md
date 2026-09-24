# promptforge-parser

This crate owns PromptForge prompt-document parsing and compiles each Lua region into a `LuaProgram` without executing it.

- General Markdown host utilities stay in the Lua host surface. Moving them here would close the parser-to-Lua dependency cycle.
- The `promptforge-engine` executor consumes this crate. This crate never imports an executor.
- Parser operations used only by `promptforge-engine` live in `detail`, which the `promptforge` facade never re-exports; they are not host API and must not gain facade status without a design change.
