//! The parsed prompt tree: [`Prompt`], its [`Section`]s, and their
//! [`Block`]s. Hosts read a prompt's title and frontmatter; the tree below
//! them is the engine's, reached through [`crate::detail`].
//!
//! Construction happens in the parsing modules; this module holds the value
//! types and the invariant-preserving operations on them.

use crate::LuaProgram;
use crate::build::Frontmatter;

/// One executable block inside a section: a compiled Lua fence or prose.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// An exact `lua` fence compiled at parse time.
    Lua(LuaProgram),
    /// Author prose: the pending Markdown accumulated since the nearest
    /// preceding heading, `lua` fence, or thematic break. The executor
    /// installs it as the following Lua block's lazy `prose` template.
    #[non_exhaustive]
    Prose {
        /// Captured Markdown, trimmed of surrounding blank lines.
        text: String,
    },
}

/// One section of a prompt: a heading, ordered blocks, and children.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Section {
    /// The heading text (the section's address).
    pub(crate) name: String,
    /// The heading level, 2 through 6.
    pub(crate) level: u8,
    /// Ordered lua/prose blocks for this section.
    pub(crate) blocks: Vec<Block>,
    /// Child sections nested under this one (deeper heading levels).
    pub(crate) children: Vec<Section>,
    /// Pre-parsed bullet items for list-only sections (no lua blocks).
    /// Empty for non-list sections.
    pub(crate) items: Vec<String>,
}

impl Section {
    /// Returns the heading text (the section's address).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the heading level (2 through 6).
    #[must_use]
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Returns the ordered Lua and prose blocks of this section.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Returns the child sections nested under this one.
    #[must_use]
    pub fn children(&self) -> &[Section] {
        &self.children
    }

    /// Returns the pre-parsed bullet items for a list-only section.
    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Classic leading Lua fence when the first block is Lua.
    #[must_use]
    pub fn prologue(&self) -> Option<&LuaProgram> {
        match self.blocks.first() {
            Some(Block::Lua(program)) => Some(program),
            _ => None,
        }
    }

    /// Text of the last prose block, or `""` when the section has none.
    #[must_use]
    pub fn prose(&self) -> &str {
        self.blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                Block::Prose { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or("")
    }

    /// Classic trailing Lua fence when the last block is Lua and not the sole
    /// leading prologue (a section that is only one Lua block has no epilog).
    #[must_use]
    pub fn epilog(&self) -> Option<&LuaProgram> {
        match self.blocks.as_slice() {
            [Block::Lua(_)] => None,
            [.., Block::Lua(program)] => Some(program),
            _ => None,
        }
    }

    /// True when this section is a validated bullet list.
    ///
    /// A section is list-only exactly when it parsed into non-empty
    /// [`items`](Self::items) - i.e. it had no Lua blocks and every nonblank
    /// prose line was a valid list item (PF-PARSER-005). Ordinary prose (even
    /// prose that happens to contain a single bullet line) is not list-only.
    #[must_use]
    pub fn is_list_only(&self) -> bool {
        !self.items.is_empty()
    }
}

/// A fully parsed prompt file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Prompt {
    /// The parsed YAML frontmatter.
    pub(crate) frontmatter: Frontmatter,
    /// The required H1 title.
    pub(crate) title: String,
    /// The compiled `lua shared` library loaded into section VMs.
    pub(crate) replay: Option<LuaProgram>,
    /// Ordered live Lua and prose blocks from the H1.
    pub(crate) h1_blocks: Vec<Block>,
    /// Human-readable prose from the H1.
    pub(crate) description_text: String,
    /// Top-level sections (H2s) in file order.
    pub(crate) sections: Vec<Section>,
}

impl Prompt {
    /// Returns the parsed frontmatter.
    #[must_use]
    pub fn frontmatter(&self) -> &Frontmatter {
        &self.frontmatter
    }

    /// Returns the required H1 title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the compiled `lua shared` library, when the prompt declares one;
    /// the engine reaches it through [`crate::detail::replay`].
    #[must_use]
    pub(crate) fn replay(&self) -> Option<&LuaProgram> {
        self.replay.as_ref()
    }

    /// Returns the ordered live Lua and prose blocks from the H1; the engine
    /// reaches them through [`crate::detail::h1_blocks`].
    #[must_use]
    pub(crate) fn h1_blocks(&self) -> &[Block] {
        &self.h1_blocks
    }

    /// Returns the top-level H2 sections in file order; the engine reaches
    /// them through [`crate::detail::sections`].
    #[must_use]
    pub(crate) fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The entry-point section: the first top-level section in file order;
    /// the engine reaches it through [`crate::detail::entry`].
    #[must_use]
    pub(crate) fn entry(&self) -> Option<&Section> {
        self.sections.first()
    }

    /// Removes the human-readable prose from the H1, keeping only its live Lua
    /// blocks.
    ///
    /// This is the invariant-preserving replacement for mutating the H1 blocks
    /// directly: it drops every prose block from the H1 and clears the
    /// derived description text, leaving the compiled H1 Lua blocks and the rest
    /// of the prompt tree untouched. Callers use it to run a prompt's live H1
    /// resolution without sending any H1 prose to a model.
    pub fn strip_h1_prose(&mut self) {
        self.h1_blocks
            .retain(|block| matches!(block, Block::Lua(_)));
        self.description_text.clear();
    }
}
