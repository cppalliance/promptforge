//! The control-markup delimiter inventory and its single-pass matcher.
//!
//! Chat templates address their models through control markup: role headers,
//! turn boundaries, tool-call envelopes, media placeholders, and document
//! boundaries. Untrusted content quoting one verbatim can forge a turn or
//! close a block the template opened, so the envelope's encoding pass spaces
//! the opener of every delimiter listed here. The inventory is ported from
//! the upstream closed name list and stays closed on purpose: `<div>`,
//! `List<String>`, `[1]`, and lowercase `[inst]` are prose and stay as
//! typed. It includes structural tokens that tokenizers do not flag as
//! special, because a string sweep keyed on the special-token flag misses
//! real delimiters.
//!
//! DeepSeek's fullwidth markers (`<｜name｜>`, U+FF5C bars) are the one open
//! name class - the family keeps adding spellings - so they match as a
//! bounded class rather than as table entries.

#[path = "inventory-table.rs"]
mod table;

pub(super) use table::{CONTROL_MARKUP, Shape};

/// The fullwidth vertical bar (U+FF5C) framing DeepSeek's open marker class.
const FULLWIDTH_BAR: char = '\u{ff5c}';

/// The most characters allowed after the first letter of a fullwidth marker
/// name, matching the upstream `{0,39}` bound.
const FULLWIDTH_NAME_MAX: usize = 39;

/// The length in bytes of the inventory delimiter opening at the start of
/// `text`, if one does.
///
/// `prev_lt` records whether the character before `text` was a `<`, which
/// anchors the doubled-angle `<<SYS>>` form on its second bracket.
pub(super) fn delimiter_len(text: &str, prev_lt: bool) -> Option<usize> {
    match text.as_bytes().first() {
        Some(b'[') => literal_len(text, b'['),
        Some(b'<') => angle_len(text, prev_lt),
        _ => None,
    }
}

/// The length of the angle-bracket delimiter at the start of `text`.
fn angle_len(text: &str, prev_lt: bool) -> Option<usize> {
    pipe_len(text)
        .or_else(|| bare_tag_len(text))
        .or_else(|| literal_len(text, b'<'))
        .or_else(|| attribute_len(text))
        .or_else(|| fullwidth_len(text))
        .or_else(|| doubled_angle_len(text, prev_lt))
}

/// The length of the `<|name|>`-family delimiter at the start of `text`.
///
/// The closer form `<|/name|>` shares the opener. The name must be followed
/// by `|>` or `>`, so `<|tool_call|>` cannot match as the shorter `tool`.
fn pipe_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("<|")?;
    let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::Pipe) {
            continue;
        }
        for name in group.names {
            let Some(after) = rest.strip_prefix(name) else {
                continue;
            };
            let term = if after.starts_with("|>") {
                2
            } else if after.starts_with('>') {
                1
            } else {
                continue;
            };
            return Some(2 + close + name.len() + term);
        }
    }
    None
}

/// The length of the `<name>`-family bare tag at the start of `text`.
fn bare_tag_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::BareTag) {
            continue;
        }
        for name in group.names {
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with('>')
            {
                return Some(1 + close + name.len() + 1);
            }
        }
    }
    None
}

/// The length of the literal opener at the start of `text` whose first byte
/// is `open`.
fn literal_len(text: &str, open: u8) -> Option<usize> {
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::Literal) {
            continue;
        }
        for lit in group.names {
            if lit.as_bytes().first() == Some(&open) && text.starts_with(lit) {
                return Some(lit.len());
            }
        }
    }
    None
}

/// The length of the `<function=...` / `<name param="...">` attribute opener
/// at the start of `text`, where the `name=` form allows any whitespace run.
fn attribute_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    for name in ["function", "parameter"] {
        if let Some(after) = rest.strip_prefix(name)
            && after.starts_with('=')
        {
            return Some(1 + name.len() + 1);
        }
    }
    for name in ["function", "parameter", "param"] {
        if let Some(after) = rest.strip_prefix(name) {
            let trimmed = after.trim_start_matches(char::is_whitespace);
            let ws = after.len() - trimmed.len();
            if ws > 0 && trimmed.starts_with("name=\"") {
                return Some(1 + name.len() + ws + "name=\"".len());
            }
        }
    }
    None
}

/// The length of the fullwidth `<｜name｜>` marker at the start of `text`.
///
/// The one open name class: an ASCII letter, then at most
/// [`FULLWIDTH_NAME_MAX`] joiner characters, then the closing bar. The
/// charset restriction keeps the class off real CJK content.
fn fullwidth_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    let mut rest = rest.strip_prefix(FULLWIDTH_BAR)?;
    let first = rest.chars().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    rest = &rest[first.len_utf8()..];
    let mut total = 1 + FULLWIDTH_BAR.len_utf8() + first.len_utf8();
    let mut extra = 0usize;
    loop {
        let c = rest.chars().next()?;
        if c == FULLWIDTH_BAR {
            let after = &rest[FULLWIDTH_BAR.len_utf8()..];
            return after
                .starts_with('>')
                .then_some(total + FULLWIDTH_BAR.len_utf8() + 1);
        }
        if extra >= FULLWIDTH_NAME_MAX || !is_fullwidth_name_char(c) {
            return None;
        }
        extra += 1;
        total += c.len_utf8();
        rest = &rest[c.len_utf8()..];
    }
}

/// Whether `c` may continue a fullwidth marker name: ASCII letters, the
/// one-quarter-block joiner DeepSeek uses, underscore, space, or backslash
/// (the upstream parser also recognizes backslash-escaped spellings).
fn is_fullwidth_name_char(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{2581}' | '_' | ' ' | '\\')
}

/// The length of the `<<SYS>>` system-block opener at the start of `text`,
/// anchored on the second bracket via `prev_lt`.
fn doubled_angle_len(text: &str, prev_lt: bool) -> Option<usize> {
    if !prev_lt {
        return None;
    }
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::DoubledAngle) {
            continue;
        }
        for name in group.names {
            let Some(rest) = text.strip_prefix('<') else {
                continue;
            };
            let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with(">>")
            {
                return Some(1 + close + name.len() + 2);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "inventory-tests.rs"]
mod tests;
