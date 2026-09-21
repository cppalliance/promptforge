//! Mistral taxonomy: the product line is the family, and `-YYMM`
//! snapshot suffixes collapse onto their canonical entry when the base
//! id is in the same list. Mistral's rules sit in this sibling module
//! so the provider file stays under the workspace's 500-line ceiling.

use gateway_api_types::ModelEntry;

use crate::taxonomy::{SnapshotStyle, collapse_variants, strip_snapshot};

/// The entry's family: the product line for a known prefix, and the
/// whole id otherwise. Longer prefixes come first so `codestral-embed`
/// and `mistral-embed` land in `embed` rather than `codestral`.
fn family_of(id: &str) -> String {
    const LINES: &[(&str, &str)] = &[
        ("mistral-embed", "embed"),
        ("codestral-embed", "embed"),
        ("mistral-ocr", "ocr"),
        ("mistral-moderation", "moderation"),
        ("mistral-medium", "mistral-medium"),
        ("mistral-small", "mistral-small"),
        ("mistral-code", "mistral-code"),
        ("mistral-vibe-cli", "mistral-vibe-cli"),
        ("codestral", "codestral"),
        ("ministral", "ministral"),
        ("magistral", "magistral"),
        ("voxtral", "voxtral"),
        ("labs-leanstral", "labs-leanstral"),
    ];
    for (prefix, family) in LINES {
        if id == *prefix || id.starts_with(&format!("{prefix}-")) {
            return (*family).to_owned();
        }
    }
    id.to_owned()
}

/// Sets every entry's family, then collapses `-YYMM` snapshot suffixes
/// onto their canonical entries.
pub(crate) fn apply(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    collapse_variants(entries, |id| strip_snapshot(id, SnapshotStyle::YearMonth));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gateway_api_types::ModelEntry;

    use super::apply;

    /// Trimmed 2026-09-14 sheet excerpt: the real Mistral ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-mistral.json");

    /// The fixture ids with the provider's taxonomy applied, by id.
    fn classified() -> BTreeMap<String, ModelEntry> {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply(&mut entries);
        entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect()
    }

    #[test]
    fn fixture_ids_classify_into_product_line_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("codestral-latest", "codestral"),
            ("codestral-2508", "codestral"),
            ("mistral-small-latest", "mistral-small"),
            ("mistral-small-2603", "mistral-small"),
            ("mistral-medium", "mistral-medium"),
            ("mistral-medium-3.5", "mistral-medium"),
            ("mistral-medium-3-5", "mistral-medium"),
            ("mistral-code-latest", "mistral-code"),
            ("mistral-code-fim-latest", "mistral-code"),
            ("mistral-vibe-cli-fast", "mistral-vibe-cli"),
            ("mistral-embed", "embed"),
            ("codestral-embed", "embed"),
            ("mistral-ocr-latest", "ocr"),
            ("mistral-ocr-4-1", "ocr"),
            ("mistral-moderation-2603", "moderation"),
            ("voxtral-mini-2602", "voxtral"),
            ("voxtral-small-latest", "voxtral"),
            ("magistral-small-latest", "magistral"),
            ("ministral-3b-2512", "ministral"),
            ("labs-leanstral-1-5", "labs-leanstral"),
            ("labs-leanstral-1-5-1", "labs-leanstral"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn year_month_snapshots_collapse_onto_present_aliases() {
        let by_id = classified();
        let table: &[(&str, &str, &str)] = &[
            ("mistral-medium-2604", "mistral-medium", "2604"),
            ("mistral-embed-2312", "mistral-embed", "2312"),
            ("codestral-embed-2505", "codestral-embed", "2505"),
        ];
        for &(id, base, suffix) in table {
            let entry = &by_id[id];
            assert_eq!(entry.variant_of.as_deref(), Some(base), "{id}");
            assert_eq!(entry.variant.as_deref(), Some(suffix), "{id}");
            assert_eq!(
                entry.family, by_id[base].family,
                "{id} inherits its canonical's family"
            );
        }
    }

    #[test]
    fn year_month_snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // `codestral-2508`, `ministral-3b-2512`, and the other dated ids
        // whose base is absent (only `-latest` aliases exist) stay canonical.
        for id in ["codestral-2508", "ministral-3b-2512", "voxtral-mini-2602"] {
            assert!(
                by_id[id].variant_of.is_none(),
                "{id} must stay canonical: its base id is not in the list"
            );
        }
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
        }
    }
}
