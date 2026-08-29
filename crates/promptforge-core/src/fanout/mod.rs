//! Explicit fanout: map a worker section over a collection of members.
//!
//! A section's Lua calls `fanout(worker, collection)` to run the worker
//! template once per collection member. The collection is any Lua table: the
//! array part (`1..=#t`) iterates in order first, then the hash part in
//! undefined order. An array member arrives as the arm's `item` value as
//! itself; a hash member arrives as a pair table (`item.key` / `item.value`).
//! A list section's pre-parsed items feed in through `list_from_section`:
//! `fanout("### Worker", list_from_section("### List"))`.
//!
//! The scheduler drives the fanout: the call yields a structural request,
//! and the driver forks one arm chain per member (at most the run's
//! `max_fanout_concurrency` active at once), joins them, and resumes the
//! caller with the ordered results. This module carries the pieces that
//! boundary shares: [`resolve_sibling`] (the exact `(level, name)` heading
//! resolution every control surface uses) and [`ArmFinalizer`] (the
//! exactly-once terminal-observation guard every arm chain carries). The
//! member-wise collection conversion at the protocol boundary lives in the
//! `promptforge-lua` crate, beside the VM and the coroutine protocol that
//! consume it.

use crate::parser::Section;
use crate::{Error, Result};

/// Parses a section heading like `"### Name"` into an exact `(level, name)`
/// address.
///
/// The marker run must be one-or-more `#`, immediately followed by whitespace,
/// then a non-empty name. `"###Name"` (no whitespace) and a bare name are both
/// rejected, so a malformed heading can never be silently reinterpreted.
///
/// # Errors
/// Returns [`Error::Lua`] when the marker run is absent, is not followed by
/// whitespace, or the name is empty.
fn parse_heading_address(heading: &str) -> Result<(usize, String)> {
    let stripped = heading.trim();
    let level = stripped.chars().take_while(|&c| c == '#').count();
    if level == 0 {
        return Err(Error::Lua(format!(
            "section heading must include ### markers, got bare name: {stripped}"
        )));
    }
    // The `#` run is ASCII, so a byte slice at `level` is a valid boundary.
    let rest = &stripped[level..];
    // Checked before the whitespace gate: a marker-only heading (`###`) has
    // no name to parse whether or not whitespace followed the markers.
    let name = rest.trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "section heading has no name: {stripped}"
        )));
    }
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return Err(Error::Lua(format!(
            "section heading must have whitespace after the {} markers: {stripped}",
            "#".repeat(level)
        )));
    }
    Ok((level, name.to_owned()))
}

/// Resolves a heading string like `"### Name"` against a caller's visible
/// sections, returning the single matching section.
///
/// The heading is parsed into an exact `(level, name)` address; a section
/// matches only when BOTH its level and name are equal. Zero matches and more
/// than one match are both rejected, so an ambiguous or level-mismatched
/// heading never resolves to an arbitrary first hit.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed (see
/// [`parse_heading_address`]), when no visible section matches the exact
/// address, or when more than one matches. The error message lists the
/// visible sections and nothing else, so the error channel cannot leak the
/// rest of the document's structure.
pub(crate) fn resolve_sibling<'a>(heading: &str, visible: &'a [Section]) -> Result<&'a Section> {
    let (level, name) = parse_heading_address(heading)?;

    let mut matches = visible
        .iter()
        .filter(|section| usize::from(section.level) == level && section.name == name);
    let Some(found) = matches.next() else {
        let available: Vec<String> = visible
            .iter()
            .map(|s| format!("{} {}", "#".repeat(s.level.into()), s.name))
            .collect();
        return Err(Error::Lua(format!(
            "section heading `{}` not found; available sections: {}",
            heading.trim(),
            available.join(", ")
        )));
    };
    if matches.next().is_some() {
        return Err(Error::Lua(format!(
            "section heading `{}` is ambiguous; more than one visible section matches {} {name}",
            heading.trim(),
            "#".repeat(level)
        )));
    }
    Ok(found)
}

mod arm;

pub(crate) use arm::ArmFinalizer;

#[cfg(test)]
mod tests;
