# promptforge-parser

This crate owns PromptForge prompt-document parsing and compiles each Lua region into a `LuaProgram` without executing it.

- General Markdown Engine functions stay with the Engine globals in `promptforge-lua`. Moving them here would close the parser-to-Lua dependency cycle.
- The `promptforge-engine` executor consumes this crate. This crate never imports an executor, apart from the doctest-only `promptforge` dev-dependency the root `AGENTS.md` excepts: its doc examples alone compile against the facade, and no unit test, integration test, or bench imports it.
- Parser operations used only by `promptforge-engine` live in `detail`, which the `promptforge` facade never re-exports; they join the facade only through a design change.
