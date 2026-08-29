//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, optional H1
//! blocks and one optional `lua shared` library fence, then H2 sections.
//! H1 and section content are alternating sequences of exact `lua` fences and
//! prose ([`Block`]). Sections nest recursively (H3 under H2, H4 under H3, and
//! so on through H6). The last prose block is marked loop-capable at parse time.
//! Classic prologue/prose/epilog is exactly `[Lua, Prose, Lua]`.
//!
//! A `---` thematic break carries two roles by position. As a section's first
//! content (only whitespace before it) it marks the section off-walk: the walk
//! skips it and it runs only when addressed. Anywhere else it is a comment
//! boundary: everything below it (until the next heading) is reader-only - no
//! Lua compiles, no prose reaches the model, no items parse from it.
//!
//! The parser does no execution. It turns bytes into a [`Prompt`] tree.
//!
//! The implementation lives in the `promptforge-parser` crate and is
//! re-exported here unchanged, so existing `promptforge_core::parser::*`
//! paths keep working.

pub use promptforge_parser::{
    Block, FileDecl, Frontmatter, MAX_TOOL_ITERATIONS, MaxToolIterations, ParseError,
    ParseErrorKind, Prompt, Section, promptforge_version,
};

pub use promptforge_lua::LuaProgram;
