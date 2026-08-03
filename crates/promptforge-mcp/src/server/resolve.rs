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
fn normalize(name: &str) -> String {
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

/// Finds the entry `requested` names.
///
/// # Errors
/// Returns the message to hand the caller when nothing matches or more than one
/// does: what was asked for, and every enabled prompt nearest first, so the
/// model can correct itself on its next call.
pub(super) fn resolve<'a>(catalog: &'a Catalog, requested: &str) -> Result<&'a Entry, String> {
    let wanted = normalize(requested);
    let mut matched = catalog
        .entries()
        .iter()
        .filter(|entry| normalize(entry.name()) == wanted);

    let first = matched.next();
    match (first, matched.next()) {
        (Some(entry), None) => Ok(entry),
        (Some(_), Some(_)) => Err(format!(
            "the prompt name \"{requested}\" matches more than one prompt, so nothing was run. {}",
            nearest_first(catalog, &wanted)
        )),
        (None, _) => Err(format!(
            "there is no prompt named \"{requested}\", so nothing was run. {}",
            nearest_first(catalog, &wanted)
        )),
    }
}

/// Every enabled prompt name, closest to `wanted` first.
fn nearest_first(catalog: &Catalog, wanted: &str) -> String {
    if catalog.is_empty() {
        return "No prompts are enabled.".to_owned();
    }
    let mut names: Vec<&str> = catalog.entries().iter().map(Entry::name).collect();
    names.sort_by(|a, b| {
        distance(&normalize(a), wanted)
            .cmp(&distance(&normalize(b), wanted))
            .then_with(|| a.cmp(b))
    });
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
    use super::{distance, normalize};

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
