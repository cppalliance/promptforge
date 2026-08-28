//! Walk-target resolution: the visible-set machinery every control transfer
//! resolves against.
//!
//! A running section may address only its visible set: its home slice minus
//! itself, plus its direct children. The parent, aunts/uncles,
//! nieces/nephews, and grandchildren are never in the set, so a resolution
//! error that lists the set cannot leak the rest of the document's
//! structure. The scheduler's chains resolve jumps, `execute` targets, and
//! fanout workers through these helpers, and the `list_from_section`
//! callback resolves through them too, so every control surface agrees on
//! what a heading may name.

use crate::fanout;
use crate::parser::Section;
use crate::{Error, Result};

/// The index of `target` in `slice`, matched on the parser-unique
/// `(level, name)` pair; `None` when the slice does not contain it.
pub(super) fn section_position(slice: &[Section], target: &Section) -> Option<usize> {
    slice
        .iter()
        .position(|s| s.level == target.level && s.name == target.name)
}

/// The caller's home slice minus the caller itself, the caller found by its
/// parser-unique `(level, name)` pair and excluded by index.
///
/// A caller that is not in the slice excludes nothing: that is the fanout
/// arm's case, whose home slice is the worker's resolution set with the
/// worker already removed, so the arm's visible set comes out as exactly
/// the home slice plus the worker's children.
pub(super) fn home_without(home: &[Section], caller: &Section) -> Vec<Section> {
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
        .chain(caller.children.iter().cloned())
        .collect()
}

/// Resolves `heading` against a caller's visible set and returns the matched
/// section's pre-parsed list items.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no visible
/// section, or matches more than one (see [`fanout::resolve_sibling`]), or
/// when the resolved section has no pre-parsed items - the error that catches
/// naming a prose section by mistake.
pub(super) fn list_items_from_visible(heading: &str, visible: &[Section]) -> Result<Vec<String>> {
    let section = fanout::resolve_sibling(heading, visible)?;
    if section.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` has no pre-parsed items",
            section.name
        )));
    }
    Ok(section.items.clone())
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
/// [`fanout::resolve_sibling`]): two visible sections sharing an address
/// error loudly as ambiguous instead of silently resolving to the first.
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
    let target = fanout::resolve_sibling(heading, &visible)?;
    if let Some(index) = section_position(&jumper.children, target) {
        return Ok(JumpTarget::Child(index));
    }
    // `target` was resolved out of the visible set built from exactly these
    // two slices, so a miss here is an internal invariant violation, not a
    // user-facing Lua error.
    section_position(siblings, target)
        .map(JumpTarget::Sibling)
        .ok_or(Error::Internal(
            "resolved jump target is absent from the jumper's sibling slice",
        ))
}
