# promptforge-lua

This crate is the sandboxed Lua runtime and its host surface: the hardened
section VM, the coroutine yield/resume protocol vocabulary, the host tables
(`store`, `models`, `tools`, `sys`, `var`, `log`, `untrusted`), and the
compiled `LuaProgram`.

## Rules

- Lua sandbox and host surface only. Markdown-to-table host functions land
  here, built directly on `pulldown-cmark`; they never land in
  `promptforge-parser`, which is a prompt-document parser (the parser
  compiles `LuaProgram` at parse time, so host functions there would close
  a parser/Lua dependency cycle).
- The crate never imports the executor: `promptforge-core`'s execute layer
  drives this crate, never the reverse. `section_vm` setup composition stays
  with the executor.
- Most of the surface is `#[doc(hidden)]` cross-crate seam for
  `promptforge-core`, not host API; it must not gain documented status
  without a design change. `LuaProgram` is the exception: it is genuine API,
  re-exported by core under its historical path.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
