//! List-only section parsing.
//!
//! A no-Lua section whose every nonblank line is a bullet marker is a list; one
//! marker classifier (`classify_list_line`) feeds both detection
//! ([`is_all_list_markers`]) and extraction ([`parse_bullet_items`]) so the two
//! can never disagree about what counts as a marker.

use super::ParseErrorKind;
use crate::{Error, Result};

/// Parses bullet items from prose in a list-only section.
///
/// A list-only section has no Lua blocks. Its prose must contain only unordered
/// (`- ` or `* `) or ordered (`N. ` or `N) `) bullet lines, with blank lines
/// ignored. Returns an error if the section contains non-list content or if the
/// items list is empty.
///
/// # Errors
/// Returns a list-classified parse error when a line is a non-list line, an
/// empty marker, or when no items were found.
pub(super) fn parse_bullet_items(prose: &str, section: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for line in prose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match classify_list_line(trimmed) {
            ListLine::Item(content) => items.push(content.to_string()),
            ListLine::EmptyMarker => {
                return Err(Error::parse(
                    ParseErrorKind::List,
                    format!("empty bullet item in list section `{section}`"),
                ));
            }
            ListLine::NotAMarker => {
                return Err(Error::parse(
                    ParseErrorKind::List,
                    format!(
                        "section `{section}` is a list section but contains non-list content: {trimmed}"
                    ),
                ));
            }
        }
    }
    if items.is_empty() {
        return Err(Error::parse(
            ParseErrorKind::List,
            format!("section `{section}` is a list section but has no items"),
        ));
    }
    Ok(items)
}

/// Returns true when `prose` is entirely list markers: every nonblank line is a
/// list marker (well-formed or empty) and there is at least one such line.
///
/// This is the list-only invariant (PF-PARSER-005): a single incidental bullet
/// line in otherwise ordinary prose leaves a non-marker line present, so the
/// section is classified as prose rather than forced through strict list
/// parsing. A section whose lines are all markers but include an empty one is
/// still a list, so [`parse_bullet_items`] reports the empty item.
pub(super) fn is_all_list_markers(prose: &str) -> bool {
    let mut saw_marker = false;
    for line in prose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match classify_list_line(trimmed) {
            ListLine::NotAMarker => return false,
            ListLine::Item(_) | ListLine::EmptyMarker => saw_marker = true,
        }
    }
    saw_marker
}

/// The classification of a single nonblank prose line as a list marker.
///
/// One classifier feeds both list *detection* (is this section a list?) and
/// *extraction* (parse its items), so the two can never disagree about what
/// counts as a marker.
enum ListLine<'a> {
    /// A well-formed marker with nonblank content (the returned item text).
    Item(&'a str),
    /// A marker with no content: `-`, `*`, `1.`, `1. `, `1)`.
    EmptyMarker,
    /// Not a list marker at all (ordinary prose).
    NotAMarker,
}

/// Classify one already-trimmed, nonblank line as a list marker.
fn classify_list_line(trimmed: &str) -> ListLine<'_> {
    // Unordered: `- item` / `* item`, or a bare `-` / `*`.
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return if rest.trim().is_empty() {
            ListLine::EmptyMarker
        } else {
            ListLine::Item(rest)
        };
    }
    if trimmed == "-" || trimmed == "*" {
        return ListLine::EmptyMarker;
    }

    // Ordered: `N. item` / `N) item`, or a bare/empty `N.` / `N. ` / `N)`.
    let digits = trimmed
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return ListLine::NotAMarker;
    }
    let after_digits = &trimmed[digits..];
    let Some(after_punct) = after_digits
        .strip_prefix('.')
        .or_else(|| after_digits.strip_prefix(')'))
    else {
        return ListLine::NotAMarker;
    };
    if after_punct.is_empty() {
        // `1.` / `1)` - a bare marker with no separating space or content.
        return ListLine::EmptyMarker;
    }
    // A valid ordered item requires a space after the marker punctuation, so
    // `1.foo` is prose, not a list item.
    if let Some(content) = after_punct.strip_prefix(' ') {
        return if content.trim().is_empty() {
            ListLine::EmptyMarker
        } else {
            ListLine::Item(content)
        };
    }
    ListLine::NotAMarker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_bare_markers_are_classified() {
        assert!(matches!(
            classify_list_line("- item"),
            ListLine::Item("item")
        ));
        assert!(matches!(
            classify_list_line("1. item"),
            ListLine::Item("item")
        ));
        assert!(matches!(classify_list_line("-"), ListLine::EmptyMarker));
        assert!(matches!(classify_list_line("1."), ListLine::EmptyMarker));
        assert!(matches!(classify_list_line("1. "), ListLine::EmptyMarker));
        assert!(matches!(classify_list_line("1)"), ListLine::EmptyMarker));
        assert!(matches!(classify_list_line("1.foo"), ListLine::NotAMarker));
        assert!(matches!(
            classify_list_line("plain prose"),
            ListLine::NotAMarker
        ));
    }

    #[test]
    fn all_markers_predicate() {
        assert!(is_all_list_markers("- a\n1. b"));
        assert!(!is_all_list_markers("- a\nplain line"));
        assert!(!is_all_list_markers("   \n  "));
    }
}
