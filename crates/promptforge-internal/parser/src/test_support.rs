//! Test-only fixtures for companion crates, gated behind the `test-support`
//! feature.
//!
//! `Section` and `Block::Prose` are `#[non_exhaustive]`, so a companion
//! crate's tests cannot construct them literally; these constructors are the
//! cross-crate seam for synthetic section trees.

use crate::{Block, BodyText, Section};

/// Builds a synthetic section with the given blocks, so each test fixture
/// states only its own deltas (a prose block) instead of restating the
/// parser's `Section` literal.
#[must_use]
pub fn synthetic_section(name: &str, level: u8, blocks: Vec<Block>) -> Section {
    Section {
        name: name.to_string(),
        level,
        blocks,
        children: Vec::new(),
        items: Vec::new(),
        markdown: BodyText::from_owned(String::new()),
    }
}

/// Builds a prose block, the one `Block` variant the executor's test
/// fixtures need to construct directly.
#[must_use]
pub fn prose_block(text: String) -> Block {
    Block::Prose {
        text: BodyText::from_owned(text),
    }
}
