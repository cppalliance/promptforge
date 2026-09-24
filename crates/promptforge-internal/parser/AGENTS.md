# promptforge-parser

This crate owns PromptForge prompt-document parsing and compiles each Lua region into a `LuaProgram` without executing it.

- General Markdown host utilities stay in the Lua host surface. Moving them here would close the parser-to-Lua dependency cycle.
- The `promptforge-api-runtime` executor consumes this crate. This crate never imports an executor.
- Hidden parser error seams used by `promptforge-api-runtime` are not host API and must not gain documented status without a design change.
