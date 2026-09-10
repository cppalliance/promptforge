//! Test-only fixtures for companion crates, gated behind the `test-support`
//! feature.
//!
//! `Section` and `Block::Prose` are `#[non_exhaustive]`, so a companion
//! crate's tests cannot construct them literally; these constructors are the
//! cross-crate seam for synthetic section trees.

use crate::{Block, Section};

/// Builds a synthetic section with the given blocks and pre-parsed items, so
/// each test fixture states only its own deltas (a prose block, a list of
/// items) instead of restating the parser's `Section` literal.
#[must_use]
pub fn synthetic_section(name: &str, level: u8, blocks: Vec<Block>, items: Vec<String>) -> Section {
    Section {
        name: name.to_string(),
        level,
        blocks,
        children: Vec::new(),
        items,
    }
}

/// Builds a prose block, the one `Block` variant the executor's test
/// fixtures need to construct directly.
#[must_use]
pub fn prose_block(text: String) -> Block {
    Block::Prose { text }
}
