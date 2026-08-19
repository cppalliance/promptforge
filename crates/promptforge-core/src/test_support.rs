//! Test-only fixtures shared across the crate's test modules.

use crate::parser::{Block, Section};

/// Builds a synthetic section with the given blocks and pre-parsed items, so
/// each test fixture states only its own deltas (a prose block, a list of
/// items) instead of restating the parser's `Section` literal.
pub(crate) fn synthetic_section(
    name: &str,
    level: u8,
    blocks: Vec<Block>,
    items: Vec<String>,
) -> Section {
    Section {
        name: name.to_string(),
        level,
        blocks,
        children: Vec::new(),
        items,
        off_walk: false,
    }
}
