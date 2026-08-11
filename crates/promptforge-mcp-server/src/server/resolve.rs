//! Turning a prompt name a calling model produced into a catalog entry.
//!
//! A model that skipped the listing tool will guess a name, so resolution is
//! lenient in one narrow way and strict in every other: case folds and `-` and
//! `_` are the same character, then the match must be exact. That leniency is
//! safe because a published name may contain neither uppercase nor a hyphen, so
//! normalizing cannot merge two legal names into one.
//!
//! A near miss is never run. Ranking a guess onto a different prompt spends
//! minutes of gateway time producing the wrong artifact, and the caller cannot
//! tell, because it gets a plausible result for a prompt it did not ask for.
//! Instead the miss comes back as an answer carrying every enabled name,
//! nearest first, which is what turns the guess into a correct second call.

use crate::catalog::{Catalog, Entry};

/// Folds the two differences resolution forgives: letter case, and `-` against
/// `_`.
pub(super) fn normalize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Why a normalized name did not resolve to exactly one entry.
///
/// Returned to the caller typed rather than flattened into prose, so the caller
/// can act on the distinction rather than parse one message to tell the two
/// apart: `NotFound` is a caller-correctable miss the handler answers with the
/// enabled names nearest first, while `Ambiguous` can only arise if the catalog
/// admitted two prompts under one normalized name - an invariant its
/// construction forbids - so the handler raises it as an internal fault instead.
pub(super) enum ResolveError {
    /// No enabled prompt normalizes to the requested name.
    NotFound,
    /// More than one enabled prompt normalizes to it - a violated catalog
    /// invariant rather than caller-correctable input.
    Ambiguous,
}

/// The one entry whose normalized name is `wanted`, or the typed reason it did
/// not resolve to exactly one.
fn find<'a>(catalog: &'a Catalog, wanted: &str) -> Result<&'a Entry, ResolveError> {
    let mut matched = catalog
        .entries()
        .iter()
        .filter(|entry| normalize(entry.name()) == wanted);
    match (matched.next(), matched.next()) {
        (Some(entry), None) => Ok(entry),
        (Some(_), Some(_)) => Err(ResolveError::Ambiguous),
        (None, _) => Err(ResolveError::NotFound),
    }
}

/// Finds the entry `requested` names.
///
/// The typed [`ResolveError`] crosses this seam intact, so the caller - not this
/// function - decides how to render or handle each reason. A miss carries no
/// rendered message here; the handler builds one from [`nearest_first`] when it
/// chooses to.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when no enabled prompt normalizes to the
/// requested name, and [`ResolveError::Ambiguous`] when more than one does.
pub(super) fn resolve<'a>(
    catalog: &'a Catalog,
    requested: &str,
) -> Result<&'a Entry, ResolveError> {
    find(catalog, &normalize(requested))
}

/// Every enabled prompt name, closest to `wanted` first.
///
/// Rendering the miss is the caller's job, so this is `pub(super)`: the handler
/// calls it to build the answer it hands back for a [`ResolveError::NotFound`].
/// Each candidate's edit distance is computed once, up front, and the sort
/// orders those precomputed keys - a decorate-sort-undecorate, so the
/// comparator never re-normalizes a name or re-walks the Levenshtein matrix on
/// a candidate it revisits.
pub(super) fn nearest_first(catalog: &Catalog, wanted: &str) -> String {
    if catalog.is_empty() {
        return "No prompts are enabled.".to_owned();
    }
    let mut ranked: Vec<(usize, &str)> = catalog
        .entries()
        .iter()
        .map(|entry| {
            let name = entry.name();
            (distance(&normalize(name), wanted), name)
        })
        .collect();
    // Ties broken lexically, so equal-distance candidates come back in a stable,
    // reproducible order rather than the catalog's.
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    let names: Vec<&str> = ranked.into_iter().map(|(_, name)| name).collect();
    format!("Enabled prompts, closest first: {}.", names.join(", "))
}

/// The Levenshtein distance between two strings, in characters.
///
/// One row of the matrix is kept rather than the whole of it, since only the
/// final number is wanted.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let next_diagonal = row[j + 1];
            row[j + 1] = (row[j + 1] + 1).min(row[j] + 1).min(diagonal + cost);
            diagonal = next_diagonal;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{distance, nearest_first, normalize};
    use crate::catalog::{Catalog, Entry};

    #[test]
    fn equal_distance_names_come_back_in_lexical_order() {
        // "bbb" and "ccc" are both three edits from "aaa", so the distance key
        // ties and the lexical tie-break decides: "bbb" before "ccc",
        // whatever order the catalog held them in.
        let catalog = Catalog::new(vec![
            Entry::broken("ccc".to_owned(), PathBuf::from("c.md"), "x"),
            Entry::broken("bbb".to_owned(), PathBuf::from("b.md"), "x"),
        ]);
        assert_eq!(
            nearest_first(&catalog, "aaa"),
            "Enabled prompts, closest first: bbb, ccc."
        );
    }

    #[test]
    fn normalization_folds_case_and_hyphens() {
        assert_eq!(normalize("Research-Person"), "research_person");
        assert_eq!(normalize("research_person"), "research_person");
    }

    #[test]
    fn distance_counts_single_edits() {
        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("echo", "echo"), 0);
        assert_eq!(distance("echo", "ech"), 1);
        assert_eq!(distance("echo", "echos"), 1);
        assert_eq!(distance("echo", "ecmo"), 1);
        assert_eq!(distance("echo", ""), 4);
    }
}
