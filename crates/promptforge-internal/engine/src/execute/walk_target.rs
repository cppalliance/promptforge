//! Walk-target resolution: the visible-set machinery every control transfer
//! resolves against, and the wider reach `require` resolves libraries in.
//!
//! A control transfer or a prompt read addresses only the running section's
//! visible set: its home slice minus itself, plus its direct children. The
//! parent, aunts/uncles, nieces/nephews, and grandchildren are never in the
//! set, so a resolution error that lists the set cannot leak the rest of
//! the document's structure. The scheduler's chains resolve jumps, `call`
//! targets, and fanout workers through these helpers, and the
//! `prompt.section` and `prompt.list` reads resolve through them too, so
//! every such surface agrees on what a heading may name. `require` alone
//! also reaches the siblings of each ancestor (see [`Reachable`]), and
//! [`resolve_library`] turns the section it names into a [`Library`] keyed
//! by the section's position. [`SlicePath`] names a walk position in the
//! shared prompt tree, so a chain and a section's `prompt` reads find
//! their slice by path and read it in place.

use crate::heading_address::{self, Reachable, Visible};
use crate::lua::{Library, LibraryBlock, LibraryKey, SectionText};
use crate::parser::{Block, Prompt, Section};
use crate::{Error, Result};

/// Where a sibling slice sits in the prompt tree: the index of each
/// ancestor section from the top level down to the slice's parent. The
/// empty path is the top-level slice. A chain names its walk position by
/// path so it borrows nothing from the tree the run shares through its
/// `Arc<Prompt>`; the path resolves to the slice on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SlicePath(Vec<usize>);

impl SlicePath {
    /// The top-level slice.
    pub(super) fn root() -> Self {
        Self::default()
    }

    /// The slice of `parent`'s children, where `parent` is the section at
    /// `index` of this slice.
    pub(super) fn child(&self, index: usize) -> Self {
        let mut path = self.0.clone();
        path.push(index);
        Self(path)
    }

    /// Resolves the path against `prompt`. A path the scheduler built is
    /// always in range; an out-of-range index resolves to the empty slice,
    /// which the walk treats as exhausted rather than panicking.
    pub(super) fn resolve<'p>(&self, prompt: &'p Prompt) -> &'p [Section] {
        let mut slice = promptforge_parser::detail::sections(prompt);
        for &index in &self.0 {
            slice = slice.get(index).map_or(&[], Section::children);
        }
        slice
    }

    /// The name of the slice's section at `index`, or the prompt's title
    /// when `index` is past the slice: a chain's section name before its
    /// first entry.
    pub(super) fn name_at<'p>(&self, prompt: &'p Prompt, index: usize) -> &'p str {
        self.resolve(prompt)
            .get(index)
            .map_or(prompt.title(), Section::name)
    }
}

/// The index of `target` in `slice`, matched on the parser-unique
/// `(level, name)` pair; `None` when the slice does not contain it.
pub(super) fn section_position(slice: &[Section], target: &Section) -> Option<usize> {
    slice
        .iter()
        .position(|s| s.level() == target.level() && s.name() == target.name())
}

/// The H1 pass's visible set: the whole top-level slice. H1 is
/// section 0, so it excludes nothing and has no children.
pub(super) fn h1_visible_sections(prompt: &Prompt) -> &[Section] {
    promptforge_parser::detail::sections(prompt)
}

/// What a prompt read sees of the section `heading` names in a caller's
/// visible set, read in place through `visible`: its own raw Markdown and
/// its pre-parsed list items, or `None` when no visible section has that
/// heading.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed or matches more
/// than one visible section (see [`heading_address::find_visible`]).
pub(super) fn read_visible(heading: &str, visible: Visible<'_>) -> Result<Option<SectionText>> {
    Ok(
        heading_address::find_visible(heading, visible)?.map(|section| SectionText {
            markdown: section.markdown().to_owned(),
            items: section
                .items()
                .iter()
                .map(|item| item.as_str().to_owned())
                .collect(),
        }),
    )
}

/// Where a jump transfers control, resolved against the jumper's visible set.
#[derive(Debug)]
pub(super) enum JumpTarget {
    /// A flat index move within the jumper's own slice.
    Sibling(usize),
    /// A descent into the jumper's child slice, starting at this index.
    Child(usize),
}

/// Resolves `heading` against the visible set of `siblings[jumper]` (the
/// sibling slice minus the jumper, plus its direct children), read in place
/// through [`Visible::of`], and classifies the target: a direct child of
/// the jumper starts a child-level walk; anything else is a sibling within
/// the jumper's own slice.
///
/// Resolution is an exact `(level, name)` match (see
/// [`heading_address::resolve_visible`]): two visible sections sharing an
/// address error loudly as ambiguous instead of silently resolving to the
/// first.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no visible
/// section, or matches more than one.
pub(super) fn resolve_jump_target(
    heading: &str,
    siblings: &[Section],
    jumper: usize,
) -> Result<JumpTarget> {
    let jumper_section = siblings.get(jumper).ok_or(Error::internal(
        "a jumping section is absent from its sibling slice",
    ))?;
    let target = heading_address::resolve_visible(heading, Visible::of(siblings, jumper))?;
    if let Some(index) = section_position(jumper_section.children(), target) {
        return Ok(JumpTarget::Child(index));
    }
    // `target` was resolved out of the visible set read from exactly these
    // two slices, so a miss here is an internal invariant violation, not a
    // user-facing Lua error.
    section_position(siblings, target)
        .map(JumpTarget::Sibling)
        .ok_or(Error::internal(
            "resolved jump target is absent from the jumper's sibling slice",
        ))
}

/// Where a `require` resolves from: the H1 pass, or a walked section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RequireOrigin {
    /// The H1 pass, which reaches every top-level section.
    H1,
    /// The section at this index of the slice at this path.
    Section(SlicePath, usize),
}

impl RequireOrigin {
    /// The position `key` names: its library section's own.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when `key` is not one [`library_key`]
    /// built.
    fn from_key(key: &LibraryKey) -> Result<Self> {
        let mut indices = key
            .identity()
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<Vec<usize>, _>>()
            .map_err(|_| Error::internal("a library key names no section position"))?;
        let index = indices
            .pop()
            .ok_or(Error::internal("a library key names no section position"))?;
        Ok(Self::Section(SlicePath(indices), index))
    }

    /// What a `require` from here can name in `prompt`.
    fn reach<'p>(&'p self, prompt: &'p Prompt) -> Reachable<'p> {
        let root = promptforge_parser::detail::sections(prompt);
        match self {
            Self::H1 => Reachable::top(root),
            Self::Section(home, index) => Reachable::of(root, &home.0, *index),
        }
    }
}

/// The key naming the section at `index` of the slice at `path`: every
/// index, dot-joined, so each position has its own.
fn library_key(path: &[usize], index: usize) -> LibraryKey {
    let indices: Vec<String> = path.iter().chain([&index]).map(usize::to_string).collect();
    LibraryKey::new(indices.join("."))
}

/// Resolves `heading` for a `require` and builds the library it names:
/// from the position `key` names when a library is loading, otherwise from
/// `origin`.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed or names no
/// section in the reach, the miss listing the reach and nothing else, and
/// [`Error::Internal`] for a key [`library_key`] did not build or a block
/// kind the Engine cannot run.
pub(super) fn resolve_library(
    prompt: &Prompt,
    origin: &RequireOrigin,
    heading: &str,
    key: Option<&LibraryKey>,
) -> Result<Library> {
    let keyed = key.map(RequireOrigin::from_key).transpose()?;
    let from = keyed.as_ref().unwrap_or(origin);
    let reached = heading_address::resolve_reachable(heading, from.reach(prompt))?;
    build_library(reached.section, library_key(&reached.path, reached.index))
}

/// The library `section` makes under `key`: each Lua block in order,
/// paired with the prose block right before it, or with an empty template
/// when no prose block comes right before it. Prose after the last Lua
/// block pairs with nothing.
///
/// # Errors
/// Returns [`Error::Internal`] for a block kind the Engine cannot run.
fn build_library(section: &Section, key: LibraryKey) -> Result<Library> {
    let mut blocks = Vec::new();
    let mut pending = "";
    for block in section.blocks() {
        match block {
            Block::Prose { text, .. } => pending = text.as_str(),
            Block::Lua(program) => blocks.push(LibraryBlock {
                program: program.clone(),
                prose: std::mem::take(&mut pending).to_owned(),
            }),
            // `Block` is `#[non_exhaustive]` across the crate seam; an
            // unlisted variant has no library form.
            _ => return Err(Error::internal("an unrecognized block kind cannot load")),
        }
    }
    Ok(Library {
        key,
        heading: heading_address::section_heading(section),
        blocks,
    })
}

#[cfg(test)]
#[path = "walk_target-tests.rs"]
mod tests;
