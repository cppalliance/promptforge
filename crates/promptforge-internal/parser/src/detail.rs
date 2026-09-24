//! Operations on the parser's host-visible types that only the engine
//! performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host.

use crate::{
    Block, Error, Frontmatter, LuaProgram, MaxToolIterations, ParseError, Prompt, Section,
};

/// Unwraps a parse failure into the internal error it classifies, so the
/// engine can map it onto its own internal error type variant for variant.
#[must_use]
pub fn parse_error_into_inner(error: ParseError) -> Error {
    error.into_inner()
}

/// Returns the compiled `lua shared` library, when the prompt declares one.
#[must_use]
pub fn replay(prompt: &Prompt) -> Option<&LuaProgram> {
    prompt.replay()
}

/// Returns the ordered live Lua and prose blocks from the H1.
#[must_use]
pub fn h1_blocks(prompt: &Prompt) -> &[Block] {
    prompt.h1_blocks()
}

/// Returns the top-level H2 sections in file order.
#[must_use]
pub fn sections(prompt: &Prompt) -> &[Section] {
    prompt.sections()
}

/// The entry-point section: the first top-level section in file order.
#[must_use]
pub fn entry(prompt: &Prompt) -> Option<&Section> {
    prompt.entry()
}

/// Returns the frontmatter's tool-loop cap in the form the engine resolves
/// against its own default.
#[must_use]
pub fn max_tool_iterations(frontmatter: &Frontmatter) -> MaxToolIterations {
    frontmatter.max_tool_iterations
}
