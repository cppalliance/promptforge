# promptforge-parser

This crate is the PromptForge prompt document parser: YAML frontmatter, the
heading/section tree, exact `lua` / `lua shared` fence splitting, and the
`ParseError`/`ParseErrorKind` vocabulary. It compiles each Lua region into a
`LuaProgram` (from `promptforge-lua`) at parse time and does no execution.

## Rules

- PromptForge prompt documents only. General markdown-to-structure utilities
  (such as a Lua-callable markdown-to-table function) must not move here;
  they belong in the `promptforge-lua` host surface. The parser compiles
  `LuaProgram` at parse time, so hosting markdown utilities here would close
  a parser/Lua dependency cycle.
- The crate never imports `promptforge-core`: core's executor consumes this
  crate, never the reverse. Its only promptforge edges are
  `promptforge-lua` (`LuaProgram`) and `promptforge-core-support`
  (`Observer`, `detail`).
- The `#[doc(hidden)]` `Error` substrate and `ParseError::into_inner` are a
  cross-crate seam for `promptforge-core`'s error substrate, not host API;
  they must not gain documented status without a design change.
- The `test-support` feature gates cross-crate test fixtures
  (`test_support`); it stays off by default and out of core's re-exports.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
