//! Walk-target resolution: the visible-set machinery every control transfer
//! resolves against.
//!
//! A running section may address only its visible set: its home slice minus
//! itself, plus its direct children. The parent, aunts/uncles,
//! nieces/nephews, and grandchildren are never in the set, so a resolution
//! error that lists the set cannot leak the rest of the document's
//! structure. The scheduler's chains resolve jumps, `call` targets, and
//! fanout workers through these helpers, and the `prompt.section` and
//! `prompt.list` reads resolve through them too, so every surface agrees on
//! what a heading may name.

use crate::heading_address;
use crate::lua::SectionText;
use crate::parser::{Prompt, Section};
use crate::{Error, Result};

/// The index of `target` in `slice`, matched on the parser-unique
/// `(level, name)` pair; `None` when the slice does not contain it.
pub(super) fn section_position(slice: &[Section], target: &Section) -> Option<usize> {
    slice
        .iter()
        .position(|s| s.level() == target.level() && s.name() == target.name())
}

/// The caller's home slice minus the caller itself, the caller found by its
/// parser-unique `(level, name)` pair and excluded by index.
fn home_without(home: &[Section], caller: &Section) -> Vec<Section> {
    let caller_index = section_position(home, caller);
    home.iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != caller_index)
        .map(|(_, section)| section.clone())
        .collect()
}

/// The sections a running section may address by heading: the caller's own
/// home slice minus the caller itself, plus the caller's direct children.
pub(super) fn visible_sections(home: &[Section], caller: &Section) -> Vec<Section> {
    home_without(home, caller)
        .into_iter()
        .chain(caller.children().iter().cloned())
        .collect()
}

/// The H1 pass's visible set: the whole top-level slice. H1 is
/// section 0, so it excludes nothing and has no children.
pub(super) fn h1_visible_sections(prompt: &Prompt) -> &[Section] {
    promptforge_parser::detail::sections(prompt)
}

/// What a prompt read sees of the section `heading` names in a caller's
/// visible set: its own raw Markdown and its pre-parsed list items, or
/// `None` when no visible section has that heading.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed or matches more
/// than one visible section (see [`heading_address::find_visible`]).
pub(super) fn read_visible(heading: &str, visible: &[Section]) -> Result<Option<SectionText>> {
    Ok(
        heading_address::find_visible(heading, visible)?.map(|section| SectionText {
            markdown: section.markdown().to_owned(),
            items: section.items().to_vec(),
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

/// Resolves `heading` against the jumper's visible set (its sibling slice
/// minus itself, plus its direct children) and classifies the target: a
/// direct child of the jumper starts a child-level walk; anything else is a
/// sibling within the jumper's own slice.
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
    jumper: &Section,
) -> Result<JumpTarget> {
    let visible = visible_sections(siblings, jumper);
    let target = heading_address::resolve_visible(heading, &visible)?;
    if let Some(index) = section_position(jumper.children(), target) {
        return Ok(JumpTarget::Child(index));
    }
    // `target` was resolved out of the visible set built from exactly these
    // two slices, so a miss here is an internal invariant violation, not a
    // user-facing Lua error.
    section_position(siblings, target)
        .map(JumpTarget::Sibling)
        .ok_or(Error::internal(
            "resolved jump target is absent from the jumper's sibling slice",
        ))
}
