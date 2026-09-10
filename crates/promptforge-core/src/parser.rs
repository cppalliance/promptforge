//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, optional H1
//! blocks and one optional `lua shared` library fence, then H2 sections.
//! H1 and section content are alternating sequences of exact `lua` fences and
//! prose ([`Block`]). Sections nest recursively (H3 under H2, H4 under H3, and
//! so on through H6). Classic prologue/prose/epilog is exactly `[Lua, Prose, Lua]`.
//!
//! Prose capture follows the pending-Markdown model: Markdown accumulates
//! after each heading or ordinary `lua` fence, and each prose block is the
//! pending buffer the following `lua` fence consumes. A `---` thematic break
//! resets the pending buffer without becoming part of the prose; it carries
//! no control-flow meaning, so content below a break parses and runs
//! normally. Markdown left after the final `lua` fence is inert trailing
//! commentary, never an error.
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
