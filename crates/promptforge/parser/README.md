# promptforge-parser

The PromptForge prompt document parser: reads one markdown file (YAML
frontmatter, a required H1, H2-H6 sections, exact `lua` and `lua shared`
fences) into a `Prompt` tree, compiling each Lua region into a
`LuaProgram` at parse time. Failures report through the classified
`ParseError`/`ParseErrorKind` vocabulary. The parser does no execution.
