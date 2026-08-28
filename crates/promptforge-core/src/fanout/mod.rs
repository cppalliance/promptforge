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
//! resolution every control surface uses), [`collection_to_items`] (the
//! member-wise collection conversion at the protocol boundary), and
//! [`ArmFinalizer`] (the exactly-once terminal-observation guard every arm
//! chain carries).

use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;

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

/// Converts fanout's collection argument into the JSON members that cross
/// into the arms, one value at a time.
///
/// The array part (`1..=#t`) iterates in order first, then the hash part in
/// undefined order. Array members convert as themselves; hash members convert
/// to `{"key": k, "value": v}` pair tables so no information is lost. Each
/// member converts individually through the same serde bridge that seeds
/// `var`, because whole-table serde cannot represent mixed tables.
///
/// # Errors
/// Returns [`Error::Lua`] when the value is not a table (the message points
/// at `list_from_section` for the list-section case), when a member is a
/// function, userdata, or thread (the error names the member's index), or
/// when a hash key is not a string, number, or boolean.
pub(crate) fn collection_to_items(lua: &Lua, collection: &Value) -> Result<Vec<serde_json::Value>> {
    let Value::Table(table) = collection else {
        return Err(Error::Lua(
            "fanout's second parameter is a collection; for a list section use list_from_section(heading)".to_owned(),
        ));
    };
    let mut items = Vec::new();
    let border = table.raw_len();
    for index in 1..=border {
        let member = table.raw_get::<Value>(index).map_err(Error::lua)?;
        items.push(member_to_json(lua, member, &index.to_string())?);
    }
    for pair in table.pairs::<Value, Value>() {
        let (key, member) = pair.map_err(Error::lua)?;
        // The array part was already emitted above, in order.
        if let Value::Integer(index) = &key
            && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
        {
            continue;
        }
        // Each scalar key converts to its JSON form and its diagnostic label
        // in one match; non-scalar keys are rejected here, so no later code
        // path can meet one.
        let (key_json, key_label) = match &key {
            Value::String(s) => {
                let s = s.to_str().map_err(Error::lua)?;
                (serde_json::Value::String(s.to_owned()), s.to_owned())
            }
            Value::Integer(i) => (serde_json::Value::from(*i), i.to_string()),
            Value::Number(n) => (
                serde_json::Number::from_f64(*n)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        Error::Lua("fanout collection key is not a finite number".to_owned())
                    })?,
                n.to_string(),
            ),
            Value::Boolean(b) => (serde_json::Value::Bool(*b), b.to_string()),
            other => {
                return Err(Error::Lua(format!(
                    "fanout collection key must be a string, number, or boolean, got {}",
                    other.type_name()
                )));
            }
        };
        let value_json = member_to_json(lua, member, &key_label)?;
        items.push(json!({ "key": key_json, "value": value_json }));
    }
    Ok(items)
}

/// Converts one collection member to JSON through the serde bridge.
///
/// Functions, userdata, and threads cannot serialize, so they are rejected at
/// the call boundary with an error naming the member's index rather than the
/// bridge's type error.
fn member_to_json(lua: &Lua, member: Value, index: &str) -> Result<serde_json::Value> {
    match &member {
        Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
            "fanout collection member at index {index} is a {}; members must be data",
            member.type_name()
        ))),
        _ => lua.from_value(member).map_err(Error::lua),
    }
}

mod arm;

pub(crate) use arm::ArmFinalizer;

#[cfg(test)]
mod tests;
